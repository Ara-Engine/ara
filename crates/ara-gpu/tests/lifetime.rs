use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource,
    acceleration::{AccelerationDesc, AccelerationInstance},
    vk,
    vulkan::{Command, VulkanDevice},
};

#[test]
fn raster_only_device_omits_ray_tracing_and_keeps_normal_gpu_work_valid() -> Result<()> {
    let cache =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/raster-device");
    let mut gpu = VulkanDevice::new_raster(&[], cache)?;
    assert!(!gpu.context().info.ray_tracing);
    assert!(
        gpu.create_acceleration_structure(&AccelerationDesc::Instances(Vec::new()))
            .is_err()
    );
    let source = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let destination = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    gpu.upload(source, bytemuck::cast_slice(&[1u32, 7, 42, 100]))?;
    gpu.upload_range(source, 4, bytemuck::cast_slice(&[9u32]))?;
    assert!(gpu.upload_range(source, 15, &[0, 0]).is_err());
    assert!(gpu.upload_range(source, u64::MAX, &[0]).is_err());
    let expected = [1u32, 9, 42, 100];
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::CopyBuffer {
        source,
        destination,
        size: 16,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(destination),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(commands.finish(), &[])?;
    let waits_before = gpu.context().host_wait_stats();
    gpu.resolve(point)?;
    let waits_after = gpu.context().host_wait_stats();
    assert_eq!(waits_after.calls, waits_before.calls + 1);
    assert!(waits_after.elapsed_since(Some(waits_before)).is_some());
    assert_eq!(
        gpu.read(destination)?,
        bytemuck::cast_slice::<_, u8>(&expected)
    );
    gpu.retire(Resource::Buffer(source))?;
    gpu.retire(Resource::Buffer(destination))?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn retired_buffer_memory_can_back_fresh_buffers_on_either_queue() -> Result<()> {
    let cache =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/buffer-reuse");
    let mut gpu = VulkanDevice::new(&[], cache)?;
    for iteration in 0..24u32 {
        let size = if iteration % 3 == 0 { 8192 } else { 4096 };
        let source = gpu.create_buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            true,
        )?;
        let destination = gpu.create_buffer(size, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let expected = vec![iteration as u8; size as usize];
        gpu.upload(source, &expected)?;
        let queue = if iteration % 2 == 0 {
            Queue::Graphics
        } else {
            Queue::Compute
        };
        let mut commands = gpu.begin(queue);
        commands.push(Command::CopyBuffer {
            source,
            destination,
            size,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(destination),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[])?;
        // Collection before completion must never recycle memory still in use.
        gpu.retire(Resource::Buffer(source))?;
        gpu.collect()?;
        gpu.resolve(point)?;
        assert_eq!(gpu.read(destination)?, expected);
        assert!(gpu.upload(source, &[0]).is_err());
        gpu.retire(Resource::Buffer(destination))?;
        gpu.collect()?;
    }
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn static_acceleration_lifetime_and_copy_contracts() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/test-gpu");
    let mut gpu = VulkanDevice::new(&[], cache)?;
    let read = gpu.create_buffer(4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let image = gpu.create_image(
        8,
        8,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED,
    )?;
    let mut invalid = gpu.begin(Queue::Graphics);
    invalid.push(Command::ReadImage {
        source: image,
        destination: read,
    });
    assert!(gpu.submit(invalid.finish(), &[]).is_err());
    let source = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let destination = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let expected = [1u32, 7, 42, 100];
    gpu.upload(source, bytemuck::cast_slice(&expected))?;
    let mut copy = gpu.begin(Queue::Graphics);
    copy.push(Command::CopyBuffer {
        source,
        destination,
        size: 16,
    });
    copy.barriers([Barrier {
        resource: Resource::Buffer(destination),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(copy.finish(), &[])?;
    gpu.resolve(point)?;
    assert_eq!(
        gpu.read(destination)?,
        bytemuck::cast_slice::<_, u8>(&expected)
    );
    gpu.retire(Resource::Buffer(read))?;
    gpu.collect()?;
    assert!(gpu.read(read).is_err());
    if gpu.context().info.ray_tracing {
        let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        let vertices = gpu.create_buffer(36, usage, true)?;
        gpu.upload(
            vertices,
            bytemuck::cast_slice(&[[-1f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
        )?;
        let indices = gpu.create_buffer(12, usage, true)?;
        gpu.upload(indices, bytemuck::cast_slice(&[0u32, 1, 2]))?;
        let blas = gpu.create_acceleration_structure(&AccelerationDesc::Triangles {
            vertices,
            vertex_count: 3,
            vertex_stride: 12,
            indices,
            index_count: 3,
        })?;
        let mut commands = gpu.begin(Queue::Compute);
        commands.push(Command::BuildAccelerationStructure(blas));
        commands.barriers([Barrier {
            resource: Resource::AccelerationStructure(blas),
            before: Access::AsBuild,
            after: Access::AsInput,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(vec![
            AccelerationInstance {
                geometry: blas,
                transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
                custom_index: 7,
                mask: 255,
            },
        ]))?;
        let mut commands = gpu.begin(Queue::Compute);
        commands.push(Command::BuildAccelerationStructure(tlas));
        commands.barriers([Barrier {
            resource: Resource::AccelerationStructure(tlas),
            before: Access::AsBuild,
            after: Access::AsRead,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        gpu.bind_acceleration_structure(tlas)?;
        assert!(gpu.retire(Resource::AccelerationStructure(blas)).is_err());
        gpu.retire(Resource::AccelerationStructure(tlas))?;
        gpu.retire(Resource::AccelerationStructure(blas))?;
        gpu.collect()?;
    }
    gpu.assert_clean()?;
    Ok(())
}
