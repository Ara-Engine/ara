//! Play surfaces (D19 Game view) on Metal: an embedded player's IOSurface ring imported
//! on the adapter's device, each shown slot adopted as a UI texture. The protocol
//! matches the Vulkan adapter's: per editor frame, import again when the generation
//! changed, `show` the newest frame and release the frame it returns.
use super::{NativeUi, UiDevice};
use anyhow::{Context, Result, ensure};
use ara_gpu::external::ImportedSurface;
use ara_gpu::shared::{Frame, SharedSurface};
use objc2::Message;

/// One embedded player's surface generation on the adapter's device, and the frame it
/// shows. Close it with [`NativeUi::close_play_surface`].
pub struct PlaySurface {
    imported: ImportedSurface,
    shown: Option<Frame>,
    texture: Option<u32>,
}

impl NativeUi {
    /// Imports `surface` (from `Surfaces::surfaces`) on this adapter's device. Fails
    /// with `ARA_GPU_EXTERNAL` when Play renders on another GPU.
    /// # Safety
    /// As the Vulkan adapter's; Metal adds no requirement.
    pub unsafe fn import_play_surface(
        &self,
        _gpu: &UiDevice,
        surface: &SharedSurface,
    ) -> Result<PlaySurface> {
        // SAFETY: the adapter owns the device for the surface's whole lifetime.
        let imported = unsafe { ImportedSurface::create(&self.device, surface)? };
        Ok(PlaySurface {
            imported,
            shown: None,
            texture: None,
        })
    }
    /// Stops showing `surface`, then destroys it. Returns the frame it showed: release
    /// it to its player (`Surfaces::release`). Call between UI frames.
    pub fn close_play_surface(&mut self, mut surface: PlaySurface) -> Result<Option<Frame>> {
        surface.hide(self)
    }
}

impl PlaySurface {
    /// The imported generation: import again when `Surfaces` reports another.
    pub fn generation(&self) -> u64 {
        self.imported.generation()
    }
    /// Size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.imported.size()
    }
    /// The frame shown, if any.
    pub fn shown(&self) -> Option<Frame> {
        self.shown
    }
    /// Shows `frame` as UI texture `id` and hands the previously shown slot back.
    /// Returns the previously shown frame: release it to its player. Call between UI
    /// frames (`NativeUi::end` waits for its submit).
    pub fn show(&mut self, ui: &mut NativeUi, id: u32, frame: Frame) -> Result<Option<Frame>> {
        ensure!(
            frame.generation == self.generation(),
            "frame of generation {} on a surface of generation {}",
            frame.generation,
            self.generation()
        );
        if self.shown == Some(frame) && self.texture == Some(id) {
            return Ok(None);
        }
        let previous = self.shown.filter(|p| *p != frame);
        // SAFETY: the UI's last submit completed; the player published `frame`.
        unsafe {
            self.imported
                .exchange(Some(frame.slot), previous.map(|p| p.slot))?;
        }
        let texture = self
            .imported
            .texture(frame.slot)
            .context("frame slot outside the surface")?
            .retain();
        if let Some(old) = self.texture.filter(|t| *t != id) {
            ui.forget_external(old);
        }
        ui.attach(id, texture)?;
        self.texture = Some(id);
        self.shown = Some(frame);
        Ok(previous)
    }
    /// Stops showing: unregisters the texture and hands the shown slot back. Returns
    /// the frame to release to its player. Call between UI frames.
    pub fn hide(&mut self, ui: &mut NativeUi) -> Result<Option<Frame>> {
        if let Some(id) = self.texture.take() {
            ui.forget_external(id);
        }
        let shown = self.shown.take();
        if let Some(frame) = shown {
            // SAFETY: the UI's last submit completed and no longer samples it.
            unsafe { self.imported.exchange(None, Some(frame.slot))? };
        }
        Ok(shown)
    }
    /// The shown frame's pixels (screenshots, verification).
    pub fn read(&mut self) -> Result<image::RgbaImage> {
        let frame = self.shown.context("no frame shown")?;
        let [width, height] = self.size();
        // SAFETY: the shown slot is acquired; its copy queue is used on this thread.
        let pixels = unsafe { self.imported.read(frame.slot)? };
        image::RgbaImage::from_raw(width, height, pixels).context("surface readback size")
    }
}

impl Drop for PlaySurface {
    fn drop(&mut self) {
        if let Some(id) = self.texture {
            eprintln!(
                "ara-editor-gpu: a play surface was dropped while UI texture {id} still shows it; \
                 close it with NativeUi::close_play_surface"
            );
        }
    }
}
