//! Startup staging transfers into device-local immutable scene storage.
use super::*;
use ara_gpu::vulkan::Command;

pub(super) fn upload_buffer(
    gpu: &mut VulkanDevice,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
) -> Result<Buffer> {
    upload_buffer_on(gpu, bytes, usage, Queue::Graphics)
}
pub(super) fn upload_buffer_on(
    gpu: &mut VulkanDevice,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
    queue: Queue,
) -> Result<Buffer> {
    let staging =
        gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(staging, bytes)?;
    if usage == vk::BufferUsageFlags::TRANSFER_SRC {
        return Ok(staging);
    }
    let buffer = gpu.create_buffer(
        bytes.len() as u64,
        usage | vk::BufferUsageFlags::TRANSFER_DST,
        false,
    )?;
    let mut recorder = gpu.begin(queue);
    recorder.push(Command::CopyBuffer {
        source: staging,
        destination: buffer,
        size: bytes.len() as u64,
    });
    let after = if usage.contains(vk::BufferUsageFlags::VERTEX_BUFFER) {
        Access::Vertex
    } else if usage.contains(vk::BufferUsageFlags::INDEX_BUFFER) {
        Access::Index
    } else {
        Access::ShaderRead
    };
    recorder.barriers([ara_gpu::Barrier {
        resource: Resource::Buffer(buffer),
        before: Access::CopyDestination,
        after,
        handoff: None,
    }]);
    let point = gpu.submit(recorder.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(staging))?;
    gpu.collect()?;
    Ok(buffer)
}
pub(super) fn upload_image(
    gpu: &mut VulkanDevice,
    size: [u32; 2],
    format: vk::Format,
    bytes: &[u8],
) -> Result<Image> {
    let image = gpu.create_image(
        size[0],
        size[1],
        format,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let source = upload_buffer(gpu, bytes, vk::BufferUsageFlags::TRANSFER_SRC)?;
    let mut recorder = gpu.begin(Queue::Graphics);
    recorder.barriers([ara_gpu::Barrier {
        resource: Resource::Image(image),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    recorder.push(Command::UploadImage {
        source,
        destination: image,
    });
    recorder.barriers([ara_gpu::Barrier {
        resource: Resource::Image(image),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(recorder.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(source))?;
    gpu.collect()?;
    Ok(image)
}
