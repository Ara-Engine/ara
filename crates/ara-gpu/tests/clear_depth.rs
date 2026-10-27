use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Queue, Resource, vk,
    vulkan::{Attachment, Command, VulkanDevice},
};

fn barrier(resource: Resource, before: Access, after: Access) -> Command {
    Command::Barrier(Barrier {
        resource,
        before,
        after,
        handoff: None,
    })
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn clear_depth_clears_only_its_tile_inside_a_depth_scope() -> Result<()> {
    let cache =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/clear-depth");
    let mut gpu = VulkanDevice::new_raster(&[], cache)?;
    let image = gpu.create_image(
        8,
        8,
        vk::Format::D32_SFLOAT,
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
    )?;
    let readback = gpu.create_buffer(8 * 8 * 4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let scope = |commands: &mut ara_gpu::vulkan::Recorder, clear: Command| {
        commands.push(Command::BeginRendering {
            colors: Vec::new(),
            depth: Some(Attachment {
                image,
                clear: Some([1.; 4]),
            }),
            size: [8, 8],
        });
        commands.push(clear);
        commands.push(Command::EndRendering);
    };
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(barrier(
        Resource::Image(image),
        Access::Undefined,
        Access::Depth,
    ));
    scope(
        &mut commands,
        Command::ClearDepth {
            origin: [2, 4],
            size: [4, 2],
            depth: 0.25,
        },
    );
    commands.push(barrier(
        Resource::Image(image),
        Access::Depth,
        Access::CopySource,
    ));
    commands.push(Command::ReadImage {
        source: image,
        destination: readback,
    });
    commands.push(barrier(
        Resource::Buffer(readback),
        Access::CopyDestination,
        Access::HostRead,
    ));
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    let bytes = gpu.read(readback)?;
    let depths: &[f32] = bytemuck::cast_slice(&bytes);
    for y in 0..8 {
        for x in 0..8 {
            let tile = (2..6).contains(&x) && (4..6).contains(&y);
            let expected = if tile { 0.25 } else { 1. };
            assert_eq!(depths[y * 8 + x], expected, "texel ({x}, {y})");
        }
    }

    // Outside a rendering scope, or past its extent, the clear is rejected.
    let mut outside = gpu.begin(Queue::Graphics);
    outside.push(Command::ClearDepth {
        origin: [0, 0],
        size: [1, 1],
        depth: 0.,
    });
    assert!(gpu.submit(outside.finish(), &[]).is_err());
    let mut beyond = gpu.begin(Queue::Graphics);
    scope(
        &mut beyond,
        Command::ClearDepth {
            origin: [6, 0],
            size: [4, 1],
            depth: 0.,
        },
    );
    assert!(gpu.submit(beyond.finish(), &[]).is_err());
    Ok(())
}
