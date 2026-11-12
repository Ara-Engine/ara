//! One portable command list per UI frame: this frame's texture uploads, the target
//! and borrowed images' transitions, then one viewport + root constants + six-vertex
//! instanced draw per batch, submitted and waited for (or, with deferred completion,
//! waited by the next frame).
use super::{NativeUi, Records};
use anyhow::{Context, Result};
use ara_gpu::commands::{Attachment, Command};
use ara_gpu::metal::MetalDevice;
use ara_gpu::{Access, Barrier, Device, Image, Queue, Resource, RootConstants, vk};

/// A barrier on one image of this queue.
fn transition(image: Image, before: Access, after: Access) -> Command {
    Command::Barrier(Barrier {
        resource: Resource::Image(image),
        before,
        after,
        handoff: None,
    })
}

/// A published storage buffer holding `bytes`, reusing the current one when large
/// enough (the previous submission is finished before the next frame writes it).
fn records(
    device: &mut MetalDevice,
    current: Option<Records>,
    bytes: &[u8],
    minimum: u64,
) -> Result<Records> {
    let needed = bytes.len().max(16) as u64;
    let records = match current {
        Some(records) if records.size >= needed => records,
        old => {
            if let Some(old) = old {
                Device::retire(device, Resource::Buffer(old.buffer))?;
            }
            let size = needed.next_power_of_two().max(minimum);
            let buffer = device.create_buffer(size, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
            let slot = device.storage_buffer(buffer)?;
            Records { buffer, slot, size }
        }
    };
    if bytes.is_empty() {
        device.upload(records.buffer, &[0; 16])?;
    } else {
        device.upload(records.buffer, bytes)?;
    }
    Ok(records)
}

impl NativeUi {
    pub(super) fn submit(&mut self) -> Result<()> {
        // The previous submission reads the records written below.
        self.finish_previous()?;
        // SAFETY: Instance is repr(C), four initialized float4s, 64 bytes, with no
        // internal or tail padding.
        let instance_bytes = unsafe {
            std::slice::from_raw_parts(
                self.instances.as_ptr().cast::<u8>(),
                self.instances.len() * 64,
            )
        };
        let instances = records(
            &mut self.device,
            self.instance_records.take(),
            instance_bytes,
            65536,
        )?;
        let mask_bytes = bytemuck::cast_slice(&self.masks);
        let masks = records(
            &mut self.device,
            self.mask_records.take(),
            mask_bytes,
            16384,
        )?;
        let target = self.target.as_ref().context("UI target")?;
        let (target_image, size, target_slot) = (target.image, target.size, target.slot.index);
        let scratch = self.scratch.as_ref().map(|s| s.image);
        let repack = self.repack.zip(self.packed.as_ref().map(|p| p.image));
        let mut rec = self.device.begin(Queue::Graphics);
        // This frame's texture uploads first, ahead of everything that samples them.
        for upload in &self.pending {
            let before = if upload.fresh {
                Access::Undefined
            } else {
                Access::ShaderRead
            };
            rec.push(transition(upload.image, before, Access::CopyDestination));
            rec.push(Command::UploadImage {
                source: upload.staging,
                destination: upload.image,
            });
            rec.push(transition(
                upload.image,
                Access::CopyDestination,
                Access::ShaderRead,
            ));
        }
        for external in self.external.values() {
            rec.push(transition(
                external.image,
                Access::CopySource,
                Access::ShaderRead,
            ));
        }
        let before = if self.target_ready {
            Access::CopySource
        } else {
            Access::Undefined
        };
        // The debug id repack (`IdSource::Repacked`): the R32F ids rewritten as packed
        // RGBA8, one texel per id texel, for the overlays below to read.
        if let Some((pass, image)) = repack {
            rec.push(transition(image, Access::Undefined, Access::Color));
            rec.push(Command::BeginRendering {
                colors: vec![Attachment {
                    image,
                    clear: Some([0.; 4]),
                }],
                depth: None,
                size: pass.size,
            });
            rec.push(Command::Pipeline(self.pipeline));
            // One full-target draw (the same root words as `draw_in`, at dpi 1: the
            // instance is in physical pixels, mode 12, no clip masks).
            rec.push(Command::Viewport {
                origin: [0, 0],
                size: pass.size,
            });
            rec.push(Command::Constants(RootConstants {
                words: [
                    instances.slot.index,
                    masks.slot.index,
                    pass.first,
                    pass.source | 12 << 16,
                    0,
                    0,
                    pass.size[0] | pass.size[1] << 16,
                    1f32.to_bits(),
                ],
            }));
            rec.push(Command::DrawProcedural {
                vertices: 6,
                instances: 1,
            });
            rec.push(Command::EndRendering);
            rec.push(transition(image, Access::Color, Access::ShaderRead));
        }
        rec.push(transition(target_image, before, Access::Color));
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: target_image,
                clear: Some([0.; 4]),
            }],
            depth: None,
            size,
        });
        rec.push(Command::Pipeline(self.pipeline));
        // One instanced draw in a clip viewport; false when the clip is empty.
        let draw_in = |rec: &mut ara_gpu::commands::Recorder,
                       clip: [u32; 4],
                       first: u32,
                       count: u32,
                       texture: u32,
                       mode: u32,
                       masks_at: [u32; 2],
                       dpi: f32| {
            let [x, y, width, height] = clip;
            // Clips are clamped to the target, so the viewport stays inside it.
            let width = width.min(size[0].saturating_sub(x));
            let height = height.min(size[1].saturating_sub(y));
            if width == 0 || height == 0 || count == 0 {
                return false;
            }
            rec.push(Command::Viewport {
                origin: [x, y],
                size: [width, height],
            });
            rec.push(Command::Constants(RootConstants {
                words: [
                    instances.slot.index,
                    masks.slot.index,
                    first,
                    texture | mode << 16,
                    masks_at[0] | masks_at[1] << 24,
                    x | y << 16,
                    width | height << 16,
                    dpi.to_bits(),
                ],
            }));
            rec.push(Command::DrawProcedural {
                vertices: 6,
                instances: count,
            });
            true
        };
        let mut passes = self.backdrops.iter().peekable();
        for (index, draw) in self.draws.iter().enumerate() {
            if let (Some(pass), Some(scratch)) =
                (passes.next_if(|pass| pass.index == index), scratch)
            {
                // Glass: pause the target, blur it horizontally into scratch, then
                // resume the target (loaded) for the vertical pass.
                rec.push(Command::EndRendering);
                rec.push(transition(target_image, Access::Color, Access::ShaderRead));
                rec.push(transition(scratch, Access::Undefined, Access::Color));
                rec.push(Command::BeginRendering {
                    colors: vec![Attachment {
                        image: scratch,
                        clear: Some([0.; 4]),
                    }],
                    depth: None,
                    size,
                });
                rec.push(Command::Pipeline(self.pipeline));
                draw_in(
                    &mut rec,
                    pass.clip,
                    pass.first,
                    1,
                    target_slot,
                    draw.mode,
                    [0, 0],
                    self.dpi,
                );
                rec.push(Command::EndRendering);
                rec.push(transition(scratch, Access::Color, Access::ShaderRead));
                rec.push(transition(target_image, Access::ShaderRead, Access::Color));
                rec.push(Command::BeginRendering {
                    colors: vec![Attachment {
                        image: target_image,
                        clear: None,
                    }],
                    depth: None,
                    size,
                });
                rec.push(Command::Pipeline(self.pipeline));
            }
            draw_in(
                &mut rec,
                draw.clip,
                draw.first,
                draw.count,
                draw.texture,
                draw.mode,
                [draw.mask_offset, draw.mask_count],
                self.dpi,
            );
        }
        rec.push(Command::EndRendering);
        rec.push(transition(target_image, Access::Color, Access::CopySource));
        for external in self.external.values() {
            rec.push(transition(
                external.image,
                Access::ShaderRead,
                Access::CopySource,
            ));
        }
        self.instance_records = Some(instances);
        self.mask_records = Some(masks);
        let point = self.device.submit(rec.finish(), &[])?;
        self.in_flight.set(Some(point));
        if !self.deferred {
            self.finish_previous()?;
        }
        // Retirement releases the staging buffers once the submission completes.
        for upload in std::mem::take(&mut self.pending) {
            Device::retire(&mut self.device, Resource::Buffer(upload.staging))?;
        }
        self.target_ready = true;
        Ok(())
    }

    /// Copy `size` pixels at `origin` of the rendered target back to the CPU.
    pub(super) fn readback(
        &mut self,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<image::RgbaImage> {
        let target = self.target.as_ref().context("UI target not rendered")?;
        anyhow::ensure!(
            self.target_ready
                && origin[0] + size[0] <= target.size[0]
                && origin[1] + size[1] <= target.size[1],
            "UI readback outside the rendered target"
        );
        let (image, full) = (target.image, target.size);
        let bytes = u64::from(full[0]) * u64::from(full[1]) * 4;
        let readback = self.device.create_readback_buffer(bytes)?;
        let mut rec = self.device.begin(Queue::Graphics);
        rec.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        let point = self.device.submit(rec.finish(), &[])?;
        self.device.resolve(point)?;
        let pixels = self.device.read(readback);
        Device::retire(&mut self.device, Resource::Buffer(readback))?;
        let pixels = pixels?;
        let row = full[0] as usize * 4;
        let mut out = Vec::with_capacity(size[0] as usize * size[1] as usize * 4);
        for y in origin[1]..origin[1] + size[1] {
            let start = y as usize * row + origin[0] as usize * 4;
            out.extend_from_slice(&pixels[start..start + size[0] as usize * 4]);
        }
        image::RgbaImage::from_raw(size[0], size[1], out).context("UI screenshot dimensions")
    }

    /// The texel at `at` of the repacked id image (`IdSource::Repacked`), bytes as stored,
    /// after the last frame: the debug source's click (wrap it in `PackedPick`). Reads the
    /// whole image back.
    pub fn repacked_texel(&mut self, at: [u32; 2]) -> Result<[u8; 4]> {
        self.wait_previous()?;
        let packed = self
            .packed
            .as_ref()
            .context("no repacked id image: render a frame with IdSource::Repacked first")?;
        let (image, size) = (packed.image, packed.size);
        anyhow::ensure!(
            at[0] < size[0] && at[1] < size[1],
            "ID pick outside the image"
        );
        let readback = self
            .device
            .create_readback_buffer(u64::from(size[0]) * u64::from(size[1]) * 4)?;
        let mut rec = self.device.begin(Queue::Graphics);
        rec.push(transition(image, Access::ShaderRead, Access::CopySource));
        rec.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        // The next frame rewrites it from Undefined, as the repack always does.
        let point = self.device.submit(rec.finish(), &[])?;
        self.device.resolve(point)?;
        let pixels = self.device.read(readback);
        Device::retire(&mut self.device, Resource::Buffer(readback))?;
        let at = (at[1] as usize * size[0] as usize + at[0] as usize) * 4;
        Ok(pixels?[at..at + 4].try_into()?)
    }
}
