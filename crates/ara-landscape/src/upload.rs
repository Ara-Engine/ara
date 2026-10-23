//! Scene-initialization uploads; never called by frame recording.
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Barrier, Buffer, Device, Image, Queue, Resource, commands::Command, vk,
    vulkan::VulkanDevice,
};

pub(crate) fn buffer(gpu: &mut VulkanDevice, bytes: &[u8]) -> Result<Buffer> {
    let target = gpu.create_buffer(
        bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        false,
    )?;
    let staging =
        gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(staging, bytes)?;
    let mut r = gpu.begin(Queue::Graphics);
    r.push(Command::CopyBuffer {
        source: staging,
        destination: target,
        size: bytes.len() as u64,
    });
    r.barriers([Barrier {
        resource: Resource::Buffer(target),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(r.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(staging))?;
    gpu.collect()?;
    Ok(target)
}
pub(crate) fn texture(
    gpu: &mut VulkanDevice,
    texture: &ara_render::scene::Texture,
) -> Result<(Image, u32)> {
    let [w, h] = texture.size;
    ensure!(
        w > 0 && h > 0 && texture.rgba.len() == w as usize * h as usize * 4,
        "invalid foliage texture pixels"
    );
    // Raw UNORM: material code owns the sRGB decode, as in ara-material.
    let image = gpu.create_image(
        w,
        h,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
    )?;
    let stage = gpu.create_buffer(
        texture.rgba.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )?;
    gpu.upload(stage, &texture.rgba)?;
    let mut r = gpu.begin(Queue::Graphics);
    r.barriers([Barrier {
        resource: Resource::Image(image),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    r.push(Command::UploadImage {
        source: stage,
        destination: image,
    });
    r.barriers([Barrier {
        resource: Resource::Image(image),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(r.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    let slot = gpu.sampled_image(image, false)?.index;
    Ok((image, slot))
}
