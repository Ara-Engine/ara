use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, vk,
    vulkan::{Command, VulkanDevice},
};
use ara_render::graph::State;
use ara_render::scene::{CameraSpec, Mesh, Vertex};
use ara_render_fidelity::{
    FidelityView, FrameInput, ImageInput, Mode, PostSettings, Settings, SunShadowSource, Tier,
    Upscaler,
};
use glam::Vec3;

fn mask(gpu: &mut VulkanDevice, value: [f32; 4]) -> Result<ImageInput> {
    let bytes = (0..65 * 37)
        .flat_map(|_| value.into_iter().flat_map(f32::to_le_bytes))
        .collect::<Vec<_>>();
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, &bytes)?;
    let image = gpu.create_image(
        65,
        37,
        vk::Format::R32G32B32A32_SFLOAT,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let mut c = gpu.begin(Queue::Graphics);
    c.barriers([
        Barrier {
            resource: Resource::Buffer(stage),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        },
        Barrier {
            resource: Resource::Image(image),
            before: Access::Undefined,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    c.push(Command::UploadImage {
        source: stage,
        destination: image,
    });
    c.barriers([Barrier {
        resource: Resource::Image(image),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(c.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    Ok(ImageInput {
        image,
        sampled_index: gpu.sampled_image(image, false)?.index,
        state: State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        },
    })
}
#[test]
fn resident_vsm_masks_sun_and_missing_pages_use_ray_visibility() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/shadow-source-cache"))?;
    let mut scene = ara_pathtrace::scenes::scene("furnace", 0);
    scene.environment.fill([0., 0., 0., 1.]);
    scene.lights.clear();
    scene.sun_direction = Vec3::Z;
    scene.sun_lux = Vec3::splat(std::f32::consts::PI);
    scene.materials[0].emissive_roughness = [0., 0., 0., 0.5];
    scene.meshes = vec![Mesh {
        entity: 1,
        name: "Sun receiver".into(),
        material: 0,
        indices: vec![0, 1, 2, 0, 2, 3],
        vertices: [[-3., -3., 0.], [3., -3., 0.], [3., 3., 0.], [-3., 3., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
            })
            .to_vec(),
    }];
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 5.),
        target: Vec3::ZERO,
        vertical_fov: 0.2,
    };
    let settings = Settings {
        render_size: [65, 37],
        display_size: [65, 37],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: Some(0.),
            ..Default::default()
        },
    };
    let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
    let mut frame = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 1. / 60.,
        reset: false,
        lighting_changed: false,
        deterministic: true,
    };
    let jitter = view.render_jitter(&frame)?;
    view.render(&mut gpu, &frame)?;
    ensure!(
        jitter == view.upscaler_input(false).jitter,
        "Upcoming jitter differs from the rendered projection"
    );
    let baseline = view.capture_radiance(&mut gpu)?[18 * 65 + 32][0];
    ensure!(baseline > 0.1, "Test sun did not illuminate receiver");
    let mut owned = Vec::new();
    for (rgb, valid, expected) in [(0., 1., 0.), (0., 0., baseline), (1., 1., baseline)] {
        let input = mask(&mut gpu, [rgb, rgb, rgb, valid])?;
        owned.push(input.image);
        ensure!(
            view.set_sun_shadow_source(SunShadowSource::VirtualShadowMap {
                visibility: input,
                render_size: [1, 1]
            })
            .is_err(),
            "Mismatched VSM size accepted"
        );
        view.set_sun_shadow_source(SunShadowSource::VirtualShadowMap {
            visibility: input,
            render_size: [65, 37],
        })?;
        for step in 0..8 {
            frame.tick += 1;
            frame.lighting_changed = step == 0;
            view.render(&mut gpu, &frame)?;
        }
        let actual = view.capture_radiance(&mut gpu)?[18 * 65 + 32][0];
        ensure!(
            (actual - expected).abs() < baseline * 0.02,
            "VSM {rgb}/{valid}: expected {expected}, got {actual}"
        );
    }
    view.destroy(&mut gpu)?;
    for image in owned {
        gpu.retire(Resource::Image(image))?;
    }
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
