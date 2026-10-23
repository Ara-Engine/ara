use anyhow::{Result, ensure};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, commands::Command, vk, vulkan::VulkanDevice,
};
use ara_render_geo::{display::DisplayOutput, settings};
use std::path::Path;

#[test]
fn reflected_viewport_settings_are_complete() {
    let mut db = ara_schema::SchemaDb::new();
    settings::register_schema(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(db.get("RenderSettings").is_some());
    assert_eq!(
        include_str!("../shaders/virtual_texture.slang"),
        ara_texture_stream::shader_source()
    );
}

#[test]
fn gpu_hdr_encoding_matches_absolute_color_reference() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join("captures/display/cache"))?;
    let colors = [
        [0., 0., 0., 1.],
        [1., 1., 1., 1.],
        [4., 2., 0.5, 1.],
        [-0.1, 0.1, 0.2, 1.],
    ];
    let input = gpu.create_image(
        4,
        1,
        vk::Format::R32G32B32A32_SFLOAT,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let stage = gpu.create_buffer(64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, bytemuck::cast_slice(&colors))?;
    let mut r = gpu.begin(Queue::Graphics);
    r.barriers([Barrier {
        resource: Resource::Image(input),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    r.push(Command::UploadImage {
        source: stage,
        destination: input,
    });
    r.barriers([Barrier {
        resource: Resource::Image(input),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let p = gpu.submit(r.finish(), &[])?;
    gpu.resolve(p)?;
    for mode in [OutputMode::Sdr, OutputMode::Hdr10, OutputMode::ScRgb] {
        let settings = OutputSettings {
            mode,
            ..Default::default()
        };
        let mut output =
            DisplayOutput::new(&mut gpu, input, [4, 1], settings, &root.join("cooked"))?;
        let list = output.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        gpu.resolve(p)?;
        for (expected, actual) in colors.iter().zip(output.encoded_pixels(&mut gpu)?) {
            let expected =
                settings.encode(settings.tone_map([expected[0], expected[1], expected[2]]));
            for i in 0..3 {
                ensure!(
                    (expected[i] - actual[i]).abs() < 0.004,
                    "{mode:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
        output.retire(&mut gpu)?;
    }
    gpu.retire(Resource::Image(input))?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
