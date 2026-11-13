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

    /// Queues the copy of the texture's texels (behind the frame just rendered) for
    /// [`Readback::take`].
    pub fn queue_read(&self, engine: &Engine) -> Readback {
        Readback::queue(engine.device(), engine.queue(), &self.texture, self.size)
    }
}

/// A 4-byte-per-texel texture copied into a mappable buffer at the time it was queued, so the
/// texels are those of the frame the copy followed even when later frames have been recorded
/// since. [`Self::take`] waits only for the copy, which rode behind a frame already finished.
pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    size: [u32; 2],
    stride: usize,
    mapped: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

/// Longest a readback is polled for before it is reported lost.
const READBACK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

impl Readback {
    /// Records the copy of `texture` (of `size`) into a new buffer and submits it.
    pub fn queue(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        size: [u32; 2],
    ) -> Self {
        let [width, height] = size;
        let row = width as usize * 4;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Scene readback"),
            size: (stride * height as usize) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
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
        let (sender, mapped) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                // The receiver lives as long as the readback.
                let _ = sender.send(result);
            });
        Self {
            buffer,
            size,
            stride,
            mapped,
        }
    }

    /// The texels, rows top first with no padding, once the copy is done (waiting for it).
    pub fn take(self, device: &wgpu::Device) -> Result<Vec<u8>> {
        // The common case: the frame finished, so the copy behind it has too. Otherwise poll
        // without waiting: a wait on the device would also wait for the frame recorded after
        // this one (and hold wgpu's device lock the render thread's submit needs).
        let started = std::time::Instant::now();
        let reported = loop {
            device
                .poll(wgpu::PollType::Poll)
                .map_err(|error| anyhow::anyhow!("polling the readback: {error:?}"))?;
            match self.mapped.try_recv() {
                Ok(reported) => break reported,
                Err(_) if started.elapsed() > READBACK_WAIT => {
                    anyhow::bail!("the readback never reported")
                }
                Err(_) => std::thread::yield_now(),
            }
        };
        reported.context("mapping the readback")?;
        let [_, height] = self.size;
        let row = self.size[0] as usize * 4;
        let mapped = self
            .buffer
            .slice(..)
            .get_mapped_range()
            .context("reading the readback")?;
        let mut texels = Vec::with_capacity(row * height as usize);
        for padded in mapped.chunks_exact(self.stride) {
            texels.extend_from_slice(&padded[..row]);
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(texels)
    }
}
