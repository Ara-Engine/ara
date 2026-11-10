//! Comparison sampling uses the v4 heap table on the native device.
use ara_gpu::{
    commands::{Attachment, Command},
    cooked::CookedPipeline,
    sampler::{CompareOp, SamplerDesc},
    *,
};

#[test]
fn comparison_table_compares_depth_on_device() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new_raster(&[], root.join("../../target/ara-gpu/comparison"))?;
    let depth = gpu.create_image(
        4,
        4,
        vk::Format::D32_SFLOAT,
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
    )?;
    let index = gpu.sampled_image(depth, true)?.index;
    let output = gpu.create_buffer(4, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    let out = gpu.storage_buffer(output)?.index;
    let cooked = root.join("tests/cooked");
    let metadata: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("comparison.json"))?)?;
    let pipeline = gpu.create_pipeline(&metadata, &cooked)?;
    let edge = |resource, before, after| Barrier {
        resource,
        before,
        after,
        handoff: None,
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([edge(
        Resource::Image(depth),
        Access::Undefined,
        Access::Depth,
    )]);
    rec.push(Command::BeginRendering {
        colors: vec![],
        depth: Some(Attachment {
            image: depth,
            clear: Some([0.5; 4]),
        }),
        size: [4, 4],
    });
    rec.push(Command::EndRendering);
    rec.barriers([edge(
        Resource::Image(depth),
        Access::Depth,
        Access::ShaderRead,
    )]);
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    for (op, expected) in [(CompareOp::Less, 1u32), (CompareOp::Greater, 0)] {
        let sampler = gpu
            .sampler(&SamplerDesc {
                compare: Some(op),
                ..Default::default()
            })?
            .index;
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::HostRead,
            Access::Storage,
        )]);
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [index, out, sampler, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::HostRead,
        )]);
        let point = gpu.submit(rec.finish(), &[])?;
        gpu.wait(point)?;
        assert_eq!(gpu.read(output)?, expected.to_le_bytes());
        gpu.assert_clean()?;
    }
    Ok(())
}
