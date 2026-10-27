//! Startup staging transfers into device-local immutable scene storage.
use super::*;
use ara_gpu::vulkan::Command;

pub(super) fn upload_buffer(
    gpu: &mut GpuDevice,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
) -> Result<Buffer> {
    upload_buffer_on(gpu, bytes, usage, Queue::Graphics)
}
pub(super) fn upload_buffer_on(
    gpu: &mut GpuDevice,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
    queue: Queue,
) -> Result<Buffer> {
    let mut allocated = Vec::new();
    let result = (|| {
        let staging =
            gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        allocated.push(Resource::Buffer(staging));
        gpu.upload(staging, bytes)?;
        if usage == vk::BufferUsageFlags::TRANSFER_SRC {
            return Ok(staging);
        }
        let buffer = gpu.create_buffer(
            bytes.len() as u64,
            usage | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?;
        allocated.push(Resource::Buffer(buffer));
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
        Ok(buffer)
    })();
    for resource in allocated {
        if result
            .as_ref()
            .ok()
            .is_none_or(|buffer| resource != Resource::Buffer(*buffer))
        {
            gpu.retire(resource)?;
        }
    }
    gpu.collect()?;
    result
}
pub(super) fn upload_image(
    gpu: &mut GpuDevice,
    size: [u32; 2],
    format: vk::Format,
    bytes: &[u8],
) -> Result<Image> {
    let mut allocated = Vec::new();
    let result = (|| {
        let image = gpu.create_image(
            size[0],
            size[1],
            format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        allocated.push(Resource::Image(image));
        let source = upload_buffer(gpu, bytes, vk::BufferUsageFlags::TRANSFER_SRC)?;
        allocated.push(Resource::Buffer(source));
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
        Ok(image)
    })();
    for resource in allocated {
        if result
            .as_ref()
            .ok()
            .is_none_or(|image| resource != Resource::Image(*image))
        {
            gpu.retire(resource)?;
        }
    }
    gpu.collect()?;
    result
}
