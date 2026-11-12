//! The Scene's id image as Renderlab writes it: the view's nearest-opaque ids packed
//! little-endian into an RGBA8 UNORM texture of the output's size (`0` nothing,
//! `0xFFFF_FFFF` a hit without an id), one texel per output pixel. The shared ring copies
//! the texture across devices; the CPU copy reads it back here.

use crate::rl::Engine;
use anyhow::{Context, Result};
use renderlab::{IdLayer, ViewId};

/// The id texture of one view size.
pub(crate) struct IdTarget {
    texture: wgpu::Texture,
    size: [u32; 2],
}

impl IdTarget {
    /// A texture Renderlab can render ids into (`RENDER_ATTACHMENT`) and the handoff can
    /// copy from (`COPY_SRC`).
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Scene ids"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { texture, size }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Writes `view`'s last frame's ids into the texture, queued behind that frame.
    pub fn write(&self, engine: &mut Engine, view: ViewId) -> Result<()> {
        engine
            .render_view_ids_to(view, &self.texture, IdLayer::Opaque)
            .context("Renderlab could not write the id image")
    }

    /// The texture's texels, rows top first with no padding, after the frame's work:
    /// waits for the copy.
    pub fn read(&self, engine: &Engine) -> Result<Vec<u8>> {
        let (device, queue) = (engine.device(), engine.queue());
        let [width, height] = self.size;
        let row = width as usize * 4;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Scene id readback"),
            size: (stride * height as usize) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                // The receiver outlives the wait below.
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow::anyhow!("waiting for the id readback: {error:?}"))?;
        receiver
            .recv()
            .context("the id readback never reported")?
            .context("mapping the id readback")?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .context("reading the id readback")?;
        let mut texels = Vec::with_capacity(row * height as usize);
        for padded in mapped.chunks_exact(stride) {
            texels.extend_from_slice(&padded[..row]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(texels)
    }
}
