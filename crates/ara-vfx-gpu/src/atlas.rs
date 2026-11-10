//! Uploads the cooked sprite atlas (`ara_vfx::atlas`) for GPU effects.
use anyhow::{Result, ensure};
use ara_gpu::{Access, Barrier, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk};
pub use ara_vfx::atlas::{ATLAS_SIZE, cook_atlas};

pub(crate) fn upload(gpu: &mut GpuDevice, bytes: &[u8]) -> Result<Image> {
    ensure!(
        bytes.len() == (ATLAS_SIZE[0] * ATLAS_SIZE[1] * 4) as usize,
        "invalid cooked sprite atlas"
    );
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let mut allocated_image = None;
    let result = (|| {
        gpu.upload(stage, bytes)?;
        let image = gpu.create_image(
            ATLAS_SIZE[0],
            ATLAS_SIZE[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        allocated_image = Some(image);
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
        Ok(image)
    })();
    gpu.retire(Resource::Buffer(stage))?;
    if result.is_err()
        && let Some(image) = allocated_image
    {
        gpu.retire(Resource::Image(image))?;
    }
    gpu.collect()?;
    result
}
