//! Native Vulkan UiGpu adapter, sharing ara-render's device and offscreen scene texture.
#![warn(missing_docs)]
mod overlay;
mod pipeline;
mod resources;
mod shared;
mod submit;
use anyhow::{Result, ensure};
use ara_gpu::{vk, vulkan::Context as Gpu};
use ara_ui::paint::Box2;
use ara_ui_render::gpu::{Batch, Clip, Instance, TextureUpload, UiGpu};
use resources::{Buffer, Image, Memory};
pub use shared::PlaySurface;
use std::collections::BTreeMap;

struct Texture {
    image: Image,
    slot: u32,
}
/// A texture copy waiting for the frame's submission: `staged[offset..]` into `image`.
struct PendingUpload {
    id: u32,
    image: vk::Image,
    size: [u32; 2],
    offset: u64,
    /// The image is new (undefined contents) rather than one being updated.
    fresh: bool,
}
#[derive(Clone, Copy)]
struct Draw {
    first: u32,
    count: u32,
    texture: u32,
    mode: u32,
    scissor: vk::Rect2D,
    mask_offset: u32,
    mask_count: u32,
}
/// Editor-only overlays rendered through the same instance ABI and clip target.
pub enum Overlay {
    /// Antialiased line segment in logical pixels.
    Line {
        /// First endpoint.
        from: [f32; 2],
        /// Second endpoint.
        to: [f32; 2],
        /// Half width in logical pixels.
        width: f32,
        /// Semantic token colour, in sRGB numeric space.
        color: [f32; 4],
    },
    /// Antialiased filled convex quad in logical pixels, corners in order (either
    /// winding); a triangle repeats a corner. Gizmo cones, plane squares and cubes.
    Quad {
        /// Corners in order around the quad.
        corners: [[f32; 2]; 4],
        /// Semantic token colour, in sRGB numeric space.
        color: [f32; 4],
    },
    /// Analytic antialiased ground grid, faded by distance and occluded by scene IDs.
    Grid {
        /// Camera position and yaw, in world metres/radians.
        camera: [f32; 4],
        /// Pitch, vertical-FOV tangent, grid spacing and fade distance.
        parameters: [f32; 4],
        /// Semantic minor-grid tint.
        color: [f32; 4],
        /// Semantic X/Z axis tints.
        axes: [[f32; 4]; 2],
    },
    /// GPU-only hover lookup; no CPU ID-buffer readback per pointer event. With an
    /// outline group table ([`NativeUi::outline_groups`]) it outlines every channel in
    /// the hovered channel's hover group.
    Hover {
        /// Normalized pointer position in the viewport.
        uv: [f32; 2],
        /// Subtle semantic hover tint.
        color: [f32; 4],
    },
    /// Silhouette from the real entity-ID texture.
    Outline {
        /// Compact render index, one-based; with an outline group table
        /// ([`NativeUi::outline_groups`]), the outline colour group: every channel in it
        /// outlines as one shape, with no edges between its channels.
        index: u32,
        /// Semantic selection or review tint.
        color: [f32; 4],
    },
}
/// Persistent GPU UI target. Native resources are independent of glyph/font/layout ownership.
///
/// Destroy this adapter before the renderer/device that created it.
pub struct NativeUi {
    memory: Memory,
    queue: vk::Queue,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    layout: vk::DescriptorSetLayout,
    descriptors: vk::DescriptorPool,
    set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    target: Option<Image>,
    target_ready: bool,
    /// Targets of other OS windows, keyed by surface; one is active at a time.
    parked: BTreeMap<u32, (Option<Image>, bool)>,
    active: u32,
    instance_buffer: Option<Buffer>,
    mask_buffer: Option<Buffer>,
    textures: BTreeMap<u32, Texture>,
    /// This frame's texture uploads: recorded at the start of its submission, so a
    /// frame with new glyphs or images waits once, not once per upload.
    pending: Vec<PendingUpload>,
    /// Their pixels, copied into `staging` at submission.
    staged: Vec<u8>,
    /// Upload staging reused across frames (every submission completes first).
    staging: Option<Buffer>,
    external: BTreeMap<u32, u32>,
    external_images: BTreeMap<u32, vk::Image>,
    free_slots: Vec<u32>,
    instances: Vec<Instance>,
    masks: Vec<[f32; 4]>,
    draws: Vec<Draw>,
    /// Time spent waiting for submitted UI work since `take_gpu_wait` (≈ GPU time,
    /// since every submission waits for its fence).
    gpu_wait: std::cell::Cell<std::time::Duration>,
    logical: [u32; 2],
    dpi: f32,
    overlays: Vec<Overlay>,
    overlay_rect: Box2,
    outline_texture: Option<u32>,
    /// UI image mapping entity-ID channels to outline groups (see `outline_groups`).
    outline_groups: Option<u32>,
}
impl NativeUi {
    /// Create an adapter on an existing device, with no secondary viewport bridge.
    /// # Safety
    /// `gpu` must outlive the adapter; all queue access is serialized on its owning thread.
    pub unsafe fn new(gpu: &Gpu) -> Result<Self> {
        let mut ui = Self {
            memory: Memory::new(gpu),
            queue: gpu.graphics,
            pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            layout: vk::DescriptorSetLayout::null(),
            descriptors: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            sampler: vk::Sampler::null(),
            target: None,
            target_ready: false,
            parked: BTreeMap::new(),
            active: 0,
            instance_buffer: None,
            mask_buffer: None,
            textures: BTreeMap::new(),
            pending: Vec::new(),
            staged: Vec::new(),
            staging: None,
            external: BTreeMap::new(),
            external_images: BTreeMap::new(),
            free_slots: (0..256).rev().collect(),
            instances: Vec::new(),
            masks: Vec::new(),
            draws: Vec::new(),
            gpu_wait: Default::default(),
            logical: [1, 1],
            dpi: 1.,
            overlays: Vec::new(),
            overlay_rect: Box2::default(),
            outline_texture: None,
            outline_groups: None,
        };
        ui.overlays = Vec::new();
        ui.initialize(gpu.info.graphics_family)?;
        ui.upload_inner(TextureUpload {
            id: 0,
            width: 1,
            height: 1,
            rgba: &[255; 4],
        })?;
        Ok(ui)
    }
    /// Current logical-to-physical pixel density.
    pub fn dpi(&self) -> f32 {
        self.dpi
    }
    /// Set the logical-to-physical scale when moving between monitors.
    pub fn set_dpi(&mut self, dpi: f32) -> Result<()> {
        ensure!(dpi.is_finite() && dpi > 0. && dpi <= 8., "Invalid UI DPI");
        self.dpi = dpi;
        Ok(())
    }
    /// Replace grid, gizmo and selection overlays when their view/model changes.
    pub fn overlays(&mut self, rect: Box2, texture: u32, overlays: Vec<Overlay>) {
        self.overlay_rect = rect;
        self.outline_texture = Some(texture);
        self.overlays = overlays;
    }
    /// Outline and hover by group instead of by channel: `table` is a UI image (uploaded
    /// through the UI renderer) whose texel `channel % 4096, channel / 4096` holds that
    /// channel's hover group in red + green × 256 and its outline colour group in blue's
    /// low 7 bits (0: none); blue's bit 7 marks ground (terrain). A group outlines as one
    /// shape: several channels that are one object (a terrain's chunks, a model's parts)
    /// show no edges between them, and ground only outlines against the sky, the void
    /// or other ground (what stands on it is in front of it). `None` outlines single
    /// channels.
    pub fn outline_groups(&mut self, table: Option<u32>) {
        self.outline_groups = table;
    }
    /// Borrow an offscreen renderer image as a UI resource; it must remain valid until replaced.
    /// The adapter transitions it from CopySource to sampled and back around its own submit.
    /// # Safety
    /// `view` and its image must belong to this device and outlive their next UI draw.
    pub unsafe fn external_image(
        &mut self,
        id: u32,
        image: vk::Image,
        view: vk::ImageView,
    ) -> Result<()> {
        let slot = if let Some(slot) = self.external.get(&id) {
            *slot
        } else {
            let slot = self
                .free_slots
                .pop()
                .ok_or_else(|| anyhow::anyhow!("UI texture heap exhausted"))?;
            self.external.insert(id, slot);
            slot
        };
        self.write_texture(slot, view);
        self.external_images.insert(id, image);
        Ok(())
    }
    /// Render into the target of another OS window (0 is the main window). Every
    /// window keeps its own target; textures, atlases and pipelines are shared.
    pub fn select_target(&mut self, surface: u32) {
        if surface == self.active {
            return;
        }
        let current = (self.target.take(), self.target_ready);
        self.parked.insert(self.active, current);
        let (target, ready) = self.parked.remove(&surface).unwrap_or((None, false));
        self.target = target;
        self.target_ready = ready;
        self.active = surface;
    }
    /// Drop a closed window's target after the queue is idle.
    pub fn release_target(&mut self, surface: u32) {
        if surface == self.active {
            self.target = None;
            self.target_ready = false;
        } else {
            self.parked.remove(&surface);
        }
    }
    /// Completed RGBA8 target, in CopySource layout, suitable for ara-platform presentation.
    pub fn output(&self) -> Result<(vk::Image, [u32; 2])> {
        let image = self
            .target
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI has not rendered"))?;
        Ok((image.image, image.size))
    }
    /// Read the actual GPU target for a screenshot or CPU-backend comparison.
    pub fn pixels(&mut self) -> Result<image::RgbaImage> {
        let size = self.output()?.1;
        self.readback([0, 0], size)
    }
    /// Read the `size` pixels at `origin` of the rendered target (the colour picker's
    /// eyedropper samples a few pixels around the pointer), clamped to the target.
    pub fn pixels_in(&mut self, origin: [u32; 2], size: [u32; 2]) -> Result<image::RgbaImage> {
        let target = self.output()?.1;
        let origin = [origin[0].min(target[0] - 1), origin[1].min(target[1] - 1)];
        let size = [
            size[0].clamp(1, target[0] - origin[0]),
            size[1].clamp(1, target[1] - origin[1]),
        ];
        self.readback(origin, size)
    }
    fn upload_inner(&mut self, texture: TextureUpload<'_>) -> Result<()> {
        ara_gpu::cpu_scope!("UI.upload");
        if self.external.contains_key(&texture.id) {
            return Ok(());
        }
        ensure!(
            texture.width > 0
                && texture.height > 0
                && texture.width as usize * texture.height as usize * 4 == texture.rgba.len(),
            "Invalid UI upload"
        );
        let size = [texture.width, texture.height];
        // A newer upload of the same texture this frame supersedes an earlier one.
        self.pending.retain(|p| p.id != texture.id);
        let offset = self.staged.len() as u64;
        self.staged.extend_from_slice(texture.rgba);
        // The same size updates the image in place: no allocation, same descriptor.
        if let Some(existing) = self.textures.get(&texture.id)
            && existing.image.size == size
        {
            self.pending.push(PendingUpload {
                id: texture.id,
                image: existing.image.image,
                size,
                offset,
                fresh: false,
            });
            return Ok(());
        }
        let image = self.memory.image(
            size,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
        )?;
        let slot = if let Some(old) = self.textures.get(&texture.id) {
            old.slot
        } else {
            self.free_slots
                .pop()
                .ok_or_else(|| anyhow::anyhow!("UI texture heap exhausted"))?
        };
        self.write_texture(slot, image.view);
        self.pending.push(PendingUpload {
            id: texture.id,
            image: image.image,
            size,
            offset,
            fresh: true,
        });
        // The replaced image is idle: every submission completes before it returns.
        self.textures.insert(texture.id, Texture { image, slot });
        Ok(())
    }
    fn write_texture(&self, slot: u32, view: vk::ImageView) {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        // SAFETY: This completed adapter uses a bounded descriptor slot and a live matching image view.
        unsafe {
            self.memory.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(1)
                    .dst_array_element(slot)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&info)],
                &[],
            );
        }
    }
}
impl UiGpu for NativeUi {
    fn upload(&mut self, texture: TextureUpload<'_>) -> ara_ui::Result<()> {
        self.upload_inner(texture).map_err(Into::into)
    }
    fn begin(&mut self, size: [u32; 2]) -> ara_ui::Result<()> {
        self.logical = size;
        self.instances.clear();
        self.masks.clear();
        self.draws.clear();
        let physical = size.map(|v| (v as f32 * self.dpi).round().max(1.) as u32);
        if self.target.as_ref().is_none_or(|t| t.size != physical) {
            self.target = Some(self.memory.image(
                physical,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            )?);
            self.target_ready = false;
        }
        Ok(())
    }
    fn draw(
        &mut self,
        batch: &Batch,
        instances: &[Instance],
        clips: &[Clip],
    ) -> ara_ui::Result<()> {
        let first = self.instances.len() as u32;
        self.instances.extend_from_slice(instances);
        let mut rect = Box2 {
            x: 0.,
            y: 0.,
            w: self.logical[0] as f32,
            h: self.logical[1] as f32,
        };
        let mut current = Some(batch.clip);
        let offset = self.masks.len() as u32;
        while let Some(index) = current {
            let clip = clips.get(index).ok_or("invalid UI clip")?;
            rect = rect.intersection(clip.rect);
            if clip.radius > 0. {
                self.masks
                    .push([clip.rect.x, clip.rect.y, clip.rect.w, clip.rect.h]);
                self.masks.push([clip.radius, 0., 0., 0.]);
            }
            current = clip.parent;
        }
        let x = (rect.x * self.dpi).floor().max(0.) as i32;
        let y = (rect.y * self.dpi).floor().max(0.) as i32;
        let right = ((rect.x + rect.w) * self.dpi).ceil() as i32;
        let bottom = ((rect.y + rect.h) * self.dpi).ceil() as i32;
        let texture = self
            .external
            .get(&batch.texture)
            .copied()
            .or_else(|| self.textures.get(&batch.texture).map(|t| t.slot))
            .ok_or("UI texture not uploaded")?;
        self.draws.push(Draw {
            first,
            count: instances.len() as u32,
            texture,
            mode: batch.pipeline as u32,
            scissor: vk::Rect2D {
                offset: vk::Offset2D { x, y },
                extent: vk::Extent2D {
                    width: (right - x).max(0) as u32,
                    height: (bottom - y).max(0) as u32,
                },
            },
            mask_offset: offset,
            mask_count: (self.masks.len() as u32 - offset) / 2,
        });
        // Scene decoration belongs immediately after the viewport image in painter order.
        // Later native controls, palettes and tooltips must occlude it.
        if batch.texture == (ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff) {
            self.append_overlays()?;
        }
        Ok(())
    }
    fn end(&mut self) -> ara_ui::Result<()> {
        self.submit().map_err(Into::into)
    }
}
impl Drop for NativeUi {
    fn drop(&mut self) {
        let device = &self.memory.device;
        // SAFETY: Device outlives the adapter by constructor contract. Queue idle precedes every teardown.
        unsafe {
            let _ = device.queue_wait_idle(self.queue);
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline, None);
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if self.descriptors != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptors, None);
            }
            if self.layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.layout, None);
            }
            if self.sampler != vk::Sampler::null() {
                device.destroy_sampler(self.sampler, None);
            }
            if self.fence != vk::Fence::null() {
                device.destroy_fence(self.fence, None);
            }
            if self.pool != vk::CommandPool::null() {
                device.destroy_command_pool(self.pool, None);
            }
        }
    }
}
