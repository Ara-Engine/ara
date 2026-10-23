//! Explicit capture-only readback of a completed native presenter image.
use anyhow::{Context, Result};
use ara_gpu::{Access, Barrier, Device, Image, Queue, Resource, commands::Command, vk};
use ara_render::{Renderer, capture::Capture};

pub fn replace(renderer: &mut Renderer, image: Image, capture: &mut Capture) -> Result<()> {
    let size = renderer.size();
    let gpu = renderer.gpu_mut();
    let read = gpu.create_buffer(
        u64::from(size[0]) * u64::from(size[1]) * 4,
        vk::BufferUsageFlags::TRANSFER_DST,
        true,
    )?;
    let mut r = gpu.begin(Queue::Graphics);
    r.push(Command::ReadImage {
        source: image,
        destination: read,
    });
    r.barriers([Barrier {
        resource: Resource::Buffer(read),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(r.finish(), &[])?;
    gpu.resolve(point)?;
    let result = gpu.read(read);
    gpu.retire(Resource::Buffer(read))?;
    gpu.collect()?;
    let image = image::RgbaImage::from_raw(size[0], size[1], result?)
        .context("presented capture dimensions")?;
    if let Some(marked) = &mut capture.marked {
        for ((marked, plain), presented) in marked
            .pixels_mut()
            .zip(capture.image.pixels())
            .zip(image.pixels())
        {
            if marked == plain {
                *marked = *presented;
            }
        }
    }
    capture.image = image;
    Ok(())
}
