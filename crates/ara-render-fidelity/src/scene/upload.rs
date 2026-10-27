//! Device-local startup/change uploads; staging retires after completion.
use super::*;
pub(crate) fn buffer(
    gpu: &mut GpuDevice,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
) -> Result<Buffer> {
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, bytes)?;
    let b = gpu.create_buffer(
        bytes.len() as u64,
        usage | vk::BufferUsageFlags::TRANSFER_DST,
        false,
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
            resource: Resource::Buffer(b),
            before: Access::Undefined,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    c.push(Command::CopyBuffer {
        source: stage,
        destination: b,
        size: bytes.len() as u64,
    });
    c.barriers([Barrier {
        resource: Resource::Buffer(b),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let p = gpu.submit(c.finish(), &[])?;
    gpu.resolve(p)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    Ok(b)
}
pub(crate) fn upload_image(
    gpu: &mut GpuDevice,
    size: [u32; 2],
    format: vk::Format,
    bytes: &[u8],
) -> Result<Image> {
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, bytes)?;
    let image = gpu.create_image(
        size[0],
        size[1],
        format,
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
    let p = gpu.submit(c.finish(), &[])?;
    gpu.resolve(p)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    Ok(image)
}
