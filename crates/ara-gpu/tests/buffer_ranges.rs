use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, commands::Command, vk, vulkan::VulkanDevice,
};

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn range_copies_preserve_other_bytes_and_reject_bad_offsets_before_submission() -> Result<()> {
    let cache =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/buffer-ranges");
    let mut gpu = VulkanDevice::new_raster(&[], cache)?;
    let source = gpu.create_buffer(64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let destination = gpu.create_buffer(64, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let input: Vec<u32> = (0..16).collect();
    gpu.upload(source, bytemuck::cast_slice(&input))?;
    let mut expected = [99u32; 16];
    gpu.upload(destination, bytemuck::cast_slice(&expected))?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([
        Barrier {
            resource: Resource::Buffer(source),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(destination),
            before: Access::HostWrite,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    commands.push(Command::CopyBufferRange {
        source,
        source_offset: 4,
        destination,
        destination_offset: 16,
        size: 12,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(destination),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    expected[4..7].copy_from_slice(&input[1..4]);
    assert_eq!(
        gpu.read(destination)?,
        bytemuck::cast_slice::<_, u8>(&expected)
    );

    for (source_offset, destination_offset, size) in [
        (0, 0, 0),
        (0, 0, 5),
        (1, 0, 4),
        (0, 2, 4),
        (60, 0, 8),
        (0, 60, 8),
        (u64::MAX - 3, 0, 8),
        (0, u64::MAX - 3, 8),
    ] {
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::CopyBufferRange {
            source,
            source_offset,
            destination,
            destination_offset,
            size,
        });
        assert!(gpu.submit(commands.finish(), &[]).is_err());
    }
    gpu.retire(Resource::Buffer(source))?;
    gpu.retire(Resource::Buffer(destination))?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
