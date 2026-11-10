//! A real storage-image handoff on the forced single-family device.
use crate::{
    Access, Barrier, Device, Handoff, Queue, Resource, RootConstants,
    commands::Command,
    cooked::CookedPipeline,
    descriptors::{Format, ImageDesc},
    texture::{ImageShape, ImageViewDesc, TextureDesc, ViewDimension},
    vk,
    vulkan::VulkanDevice,
};
fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
pub(super) fn run(gpu: &mut VulkanDevice, deferred: bool) -> anyhow::Result<()> {
    if deferred {
        gpu.begin_deferred_completion()?;
        anyhow::ensure!(
            gpu.native_queue(Queue::Graphics) == gpu.native_queue(Queue::Compute),
            "forced-family scene queues did not alias"
        );
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/feature_cooked");
    let cooked: CookedPipeline = serde_json::from_slice(&std::fs::read(root.join("views.json"))?)?;
    let pipeline = gpu.create_pipeline(&cooked, &root)?;
    let desc = TextureDesc {
        image: ImageDesc {
            size: [2, 2],
            format: Format::Rgba32Uint,
            sampled: true,
            storage: true,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            layers: 2,
            ..Default::default()
        },
    };
    let source = gpu.create_texture(&desc)?;
    let target = gpu.create_texture(&desc)?;
    let consumer = gpu.create_texture(&desc)?;
    let view = ImageViewDesc {
        dimension: ViewDimension::D2Array,
        base_mip: 0,
        mip_count: 1,
        base_layer: 0,
        layer_count: 2,
    };
    let read = gpu.sampled_view(source, view)?;
    let write = gpu.storage_view(target, view)?;
    let sampled = gpu.sampled_view(target, view)?;
    let consumed = gpu.storage_view(consumer, view)?;
    let expected: Vec<u32> = (0..32).map(|i| 0x1000 + i * 17).collect();
    let bytes = bytemuck::cast_slice(&expected);
    let staging =
        gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(staging, bytes)?;
    let output = gpu.create_readback_buffer(bytes.len() as u64)?;
    let mut compute = gpu.begin(Queue::Compute);
    compute.barriers([
        barrier(
            Resource::Buffer(staging),
            Access::HostWrite,
            Access::CopySource,
        ),
        barrier(
            Resource::Image(source),
            Access::Undefined,
            Access::CopyDestination,
        ),
        barrier(Resource::Image(target), Access::Undefined, Access::Storage),
    ]);
    compute.push(Command::UploadImage {
        source: staging,
        destination: source,
    });
    compute.barriers([barrier(
        Resource::Image(source),
        Access::CopyDestination,
        Access::ShaderRead,
    )]);
    compute.push(Command::Pipeline(pipeline));
    compute.push(Command::Constants(RootConstants {
        words: [read.index, write.index, 0, 0, 0, 0, 0, 0],
    }));
    compute.push(Command::Dispatch([2, 2, 2]));
    let edge = Barrier {
        resource: Resource::Image(target),
        before: Access::Storage,
        after: Access::ShaderRead,
        handoff: Some(Handoff {
            from: Queue::Compute,
            to: Queue::Graphics,
            release: true,
        }),
    };
    compute.barriers([edge]);
    let produced = gpu.submit(compute.finish(), &[])?;
    let mut graphics = gpu.begin(Queue::Graphics);
    graphics.barriers([Barrier {
        handoff: Some(Handoff {
            from: Queue::Compute,
            to: Queue::Graphics,
            release: false,
        }),
        ..edge
    }]);
    graphics.barriers([barrier(
        Resource::Image(consumer),
        Access::Undefined,
        Access::Storage,
    )]);
    graphics.push(Command::Pipeline(pipeline));
    graphics.push(Command::Constants(RootConstants {
        words: [sampled.index, consumed.index, 0, 0, 0, 0, 0, 0],
    }));
    graphics.push(Command::Dispatch([2, 2, 2]));
    graphics.barriers([barrier(
        Resource::Image(consumer),
        Access::Storage,
        Access::CopySource,
    )]);
    graphics.push(Command::ReadImage {
        source: consumer,
        destination: output,
    });
    graphics.barriers([barrier(
        Resource::Buffer(output),
        Access::CopyDestination,
        Access::HostRead,
    )]);
    let point = gpu.submit(graphics.finish(), &[produced])?;
    if deferred {
        let mut completion = gpu.end_deferred_completion()?;
        gpu.wait_completion(&mut completion)?;
    } else {
        gpu.wait(point)?;
    }
    anyhow::ensure!(
        gpu.read(output)? == bytes,
        "shared-family storage image handoff read stale data"
    );
    gpu.retire_pipeline(pipeline)?;
    for image in [source, target, consumer] {
        gpu.retire(Resource::Image(image))?;
    }
    for buffer in [staging, output] {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
