//! The editor UI adapter on Metal: the `UiGpu` renderer over the backend's portable
//! API, on a `MetalDevice` the adapter owns (a second device of the shared GPU, like
//! the player's loading split). It draws the cooked `ui` pipeline
//! (`shaders/ui_bindless.slang`, the engine's bindless ABI), samples renderer images it
//! adopts (`MetalDevice::adopt_texture`) and Play surfaces it imports
//! (`metal::external`), and submits once per frame and waits, like the Vulkan adapter.
//! Each draw's viewport is its clip rect: the shader maps positions into it, so the
//! viewport clips exactly as the Vulkan adapter's scissor does.
mod overlay;
mod shared;
mod submit;
use crate::Overlay;
use anyhow::{Context, Result, ensure};
use ara_gpu::metal::MetalDevice;
use ara_gpu::{Buffer, Device, HeapIndex, Image, NativeImage, Pipeline, Resource, cooked, vk};
use ara_platform::{Present, Window};
use ara_ui::paint::Box2;
use ara_ui_render::gpu::{Batch, Clip, Instance, TextureUpload, UiGpu};
pub use shared::PlaySurface;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;

/// On Metal the adapter owns its device; this marker stands where Vulkan callers pass
/// their presentation context.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiDevice;

/// The UI device marker for a renderer's device.
pub fn ui_device(_gpu: &ara_gpu::GpuDevice) -> &'static UiDevice {
    &UiDevice
}

/// The UI device marker for windows without a renderer (Hub, splash).
pub fn open_presentation(_extensions: &[CString]) -> Result<UiDevice> {
    Ok(UiDevice)
}

/// A UI texture: its image and bindless slot.
struct Texture {
    image: Image,
    slot: HeapIndex,
    size: [u32; 2],
}
/// A borrowed renderer or Play image: the native texture (to skip re-adoption of the
/// same one), its adopted image and slot.
struct External {
    texture: NativeImage,
    image: Image,
    slot: HeapIndex,
}
/// A texture copy waiting for the frame's submission.
struct Upload {
    image: Image,
    staging: Buffer,
    /// The image is new (undefined contents) rather than one being updated.
    fresh: bool,
}
#[derive(Clone, Copy)]
struct Draw {
    first: u32,
    count: u32,
    texture: u32,
    mode: u32,
    /// Physical clip rect: x, y, width, height.
    clip: [u32; 4],
    mask_offset: u32,
    mask_count: u32,
}
struct Target {
    image: Image,
    size: [u32; 2],
}
/// A host-visible storage buffer published to the bindless heap, grown as needed.
struct Records {
    buffer: Buffer,
    slot: HeapIndex,
    size: u64,
}

/// Persistent GPU UI target. Native resources are independent of glyph/font/layout
/// ownership. The adapter owns its device, so it may outlive the renderer.
pub struct NativeUi {
    device: MetalDevice,
    pipeline: Pipeline,
    target: Option<Target>,
    target_ready: bool,
    /// Targets of other OS windows, keyed by surface; one is active at a time.
    parked: BTreeMap<u32, (Option<Target>, bool)>,
    active: u32,
    instance_records: Option<Records>,
    mask_records: Option<Records>,
    textures: BTreeMap<u32, Texture>,
    /// This frame's texture uploads, recorded at the start of its submission.
    pending: Vec<Upload>,
    external: BTreeMap<u32, External>,
    /// External images the host forgot: their draws are skipped until re-attached.
    forgotten: BTreeSet<u32>,
    instances: Vec<Instance>,
    masks: Vec<[f32; 4]>,
    draws: Vec<Draw>,
    /// Time spent waiting for submitted UI work since `take_gpu_wait`.
    gpu_wait: std::cell::Cell<std::time::Duration>,
    logical: [u32; 2],
    dpi: f32,
    overlays: Vec<Overlay>,
    overlay_rect: Box2,
    outline_texture: Option<u32>,
    outline_groups: Option<u32>,
}

impl NativeUi {
    /// Create an adapter with its own device on the system GPU and the cooked UI
    /// pipeline. Fails until the `ui` pipeline is cooked with Metal artifacts.
    /// # Safety
    /// Mirrors the Vulkan contract; Metal adds no requirement.
    pub unsafe fn new(_gpu: &UiDevice) -> Result<Self> {
        let mut device = MetalDevice::new_raster(&[], std::env::temp_dir().join("ara-ui"))?;
        let dir = cooked::resource_dir(
            "ara-editor-gpu",
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let manifest: cooked::CookedPipeline = serde_json::from_slice(
            &std::fs::read(dir.join("ui.json")).with_context(|| {
                format!(
                    "the UI pipeline is not cooked ({}); cook crates/ara-editor-gpu/shaders/pipelines.json",
                    dir.display()
                )
            })?,
        )?;
        let pipeline = device.create_pipeline(&manifest, &dir)?;
        let mut ui = Self {
            device,
            pipeline,
            target: None,
            target_ready: false,
            parked: BTreeMap::new(),
            active: 0,
            instance_records: None,
            mask_records: None,
            textures: BTreeMap::new(),
            pending: Vec::new(),
            external: BTreeMap::new(),
            forgotten: BTreeSet::new(),
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
    /// Outline and hover by group instead of by channel (see the Vulkan adapter).
    pub fn outline_groups(&mut self, table: Option<u32>) {
        self.outline_groups = table;
    }
    /// Borrow a renderer image as UI texture `id`; it stays valid while attached (the
    /// adapter retains the texture). Re-attaching the same texture is free.
    /// # Safety
    /// `image` is a completed colour image of this GPU.
    pub unsafe fn external_image(
        &mut self,
        id: u32,
        image: NativeImage,
        _view: ara_gpu::NativeView,
    ) -> Result<()> {
        self.attach(id, image)
    }
    /// Adopt `texture` as UI texture `id`, replacing what `id` showed.
    fn attach(&mut self, id: u32, texture: NativeImage) -> Result<()> {
        self.forgotten.remove(&id);
        if self
            .external
            .get(&id)
            .is_some_and(|e| NativeImage::as_ptr(&e.texture) == NativeImage::as_ptr(&texture))
        {
            return Ok(());
        }
        let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC;
        let image = self.device.adopt_texture(texture.clone(), usage)?;
        let slot = self.device.sampled_image(image, false)?;
        if let Some(old) = self.external.insert(
            id,
            External {
                texture,
                image,
                slot,
            },
        ) {
            Device::retire(&mut self.device, Resource::Image(old.image))?;
        }
        Ok(())
    }
    /// Unregister an external image; its draws are skipped until attached again.
    pub fn forget_external(&mut self, id: u32) {
        self.forgotten.insert(id);
        if let Some(old) = self.external.remove(&id) {
            // A retire error leaves a stale handle that later frames never reference.
            let _ = Device::retire(&mut self.device, Resource::Image(old.image));
        }
    }
    /// Render into the target of another OS window (0 is the main window).
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
    /// Drop a closed window's target.
    pub fn release_target(&mut self, surface: u32) {
        let target = if surface == self.active {
            self.target_ready = false;
            self.target.take()
        } else {
            self.parked.remove(&surface).and_then(|(target, _)| target)
        };
        if let Some(target) = target {
            let _ = Device::retire(&mut self.device, Resource::Image(target.image));
        }
    }
    /// Completed RGBA8 target on the adapter's device, in CopySource.
    pub fn output(&self) -> Result<(Image, [u32; 2])> {
        let target = self.target.as_ref().context("UI has not rendered")?;
        Ok((target.image, target.size))
    }
    /// Read the actual GPU target for a screenshot or CPU-backend comparison.
    pub fn pixels(&mut self) -> Result<image::RgbaImage> {
        let size = self.output()?.1;
        self.readback([0, 0], size)
    }
    /// Read the `size` pixels at `origin` of the rendered target, clamped to it.
    pub fn pixels_in(&mut self, origin: [u32; 2], size: [u32; 2]) -> Result<image::RgbaImage> {
        let target = self.output()?.1;
        let origin = [origin[0].min(target[0] - 1), origin[1].min(target[1] - 1)];
        let size = [
            size[0].clamp(1, target[0] - origin[0]),
            size[1].clamp(1, target[1] - origin[1]),
        ];
        self.readback(origin, size)
    }
    /// Time spent waiting for UI submissions since the last call.
    pub fn take_gpu_wait(&self) -> std::time::Duration {
        self.gpu_wait.take()
    }
    /// The Vulkan adapter's deferred completion (`wait_previous` at the start of
    /// the next UI frame); Metal still waits each submission, so it has no effect.
    pub fn set_deferred_completion(&mut self, _deferred: bool) -> Result<()> {
        Ok(())
    }
    /// Nothing to wait: every Metal submission completes before returning.
    pub fn wait_previous(&self) -> Result<()> {
        Ok(())
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
        let superseded: Vec<_> = self
            .pending
            .iter()
            .filter(|p| {
                self.textures
                    .get(&texture.id)
                    .is_some_and(|t| t.image == p.image)
            })
            .map(|p| p.staging)
            .collect();
        self.pending.retain(|p| !superseded.contains(&p.staging));
        for staging in superseded {
            Device::retire(&mut self.device, Resource::Buffer(staging))?;
        }
        let staging = self.device.create_buffer(
            texture.rgba.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )?;
        self.device.upload(staging, texture.rgba)?;
        // The same size updates the image in place: same slot, no allocation.
        if let Some(existing) = self.textures.get(&texture.id)
            && existing.size == size
        {
            self.pending.push(Upload {
                image: existing.image,
                staging,
                fresh: false,
            });
            return Ok(());
        }
        let image = self.device.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
        )?;
        let slot = self.device.sampled_image(image, false)?;
        self.pending.push(Upload {
            image,
            staging,
            fresh: true,
        });
        if let Some(old) = self
            .textures
            .insert(texture.id, Texture { image, slot, size })
        {
            Device::retire(&mut self.device, Resource::Image(old.image))?;
        }
        Ok(())
    }
    /// The bindless slot of UI texture `id`, external or uploaded.
    fn slot(&self, id: u32) -> Option<u32> {
        self.external
            .get(&id)
            .map(|e| e.slot.index)
            .or_else(|| self.textures.get(&id).map(|t| t.slot.index))
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
            let image = self.device.create_image(
                physical[0],
                physical[1],
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            )?;
            if let Some(old) = self.target.replace(Target {
                image,
                size: physical,
            }) {
                Device::retire(&mut self.device, Resource::Image(old.image))?;
            }
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
        let Some(texture) = self.slot(batch.texture) else {
            // An external image the host forgot draws nothing this frame; a texture
            // never uploaded is a bug.
            if self.forgotten.contains(&batch.texture) {
                return Ok(());
            }
            return Err(format!("UI texture {} not uploaded", batch.texture).into());
        };
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
        let x = (rect.x * self.dpi).floor().max(0.) as u32;
        let y = (rect.y * self.dpi).floor().max(0.) as u32;
        let right = ((rect.x + rect.w) * self.dpi).ceil().max(0.) as u32;
        let bottom = ((rect.y + rect.h) * self.dpi).ceil().max(0.) as u32;
        self.draws.push(Draw {
            first,
            count: instances.len() as u32,
            texture,
            mode: batch.pipeline as u32,
            clip: [x, y, right.saturating_sub(x), bottom.saturating_sub(y)],
            mask_offset: offset,
            mask_count: (self.masks.len() as u32 - offset) / 2,
        });
        // Scene decoration belongs immediately after the viewport image in painter
        // order; later native controls, palettes and tooltips must occlude it.
        if batch.texture == (ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff) {
            self.append_overlays()?;
        }
        Ok(())
    }
    fn end(&mut self) -> ara_ui::Result<()> {
        self.submit().map_err(Into::into)
    }
}

/// Window presentation of the UI target, the same calls on every backend.
impl NativeUi {
    /// Create presentation for `window` on the adapter's device.
    /// # Safety
    /// Keep the window alive until [`Self::retire_present`]; call on its thread.
    pub unsafe fn create_present(&self, _gpu: &UiDevice, window: &Window) -> Result<Present> {
        // SAFETY: the adapter's device outlives the presentation by caller contract.
        unsafe { Present::create(&self.device, window) }
    }
    /// Present the last completed UI target.
    /// # Safety
    /// `present` was created by [`Self::create_present`].
    pub unsafe fn present(&self, _gpu: &UiDevice, present: &mut Present) -> Result<()> {
        let (image, size) = self.output()?;
        // SAFETY: the UI submission completed; the target is a transfer source.
        unsafe { present.present_image(&self.device, image, size) }
    }
    /// Release window presentation.
    /// # Safety
    /// `present` was created by [`Self::create_present`] and is not used again.
    pub unsafe fn retire_present(&self, _gpu: &UiDevice, present: &mut Present) {
        // SAFETY: forwarded caller contract.
        unsafe { present.retire(&self.device) }
    }
    /// Publish the last completed UI target into a shared-surface ring of the same GPU
    /// (an embedded game's UI composite); the copy follows the adapter's own work.
    /// # Safety
    /// `ring` belongs to a device of this GPU; this thread owns its copy queue.
    pub unsafe fn publish(
        &self,
        _gpu: &UiDevice,
        ring: &mut ara_gpu::external::ExportRing,
        tick: u64,
    ) -> Result<Option<ara_gpu::shared::Frame>> {
        let (image, size) = self.output()?;
        // SAFETY: the UI submission completed; the target is a colour image here.
        Ok(unsafe { ring.publish_image(&self.device, image, size, tick)? })
    }
    /// Wait until the adapter's device is idle.
    pub fn wait_idle(&self, _gpu: &UiDevice) -> Result<()> {
        Ok(self.device.wait_idle()?)
    }
}
