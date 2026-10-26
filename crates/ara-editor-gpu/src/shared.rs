//! Play surfaces (D19 Game view): an embedded player's shared ring imported on the
//! editor's device and shown as a UI texture through the external-image path.
//!
//! Per editor frame, after `Surfaces::poll` (ara-play-host): import again when the
//! generation changed, `take` the newest frame, [`PlaySurface::show`] it and release
//! the frame `show` returns. Between UI submits a shown slot rests in
//! `TRANSFER_SRC_OPTIMAL` on the editor's graphics family, which is what the
//! adapter's external-image barriers expect.
use crate::NativeUi;
use anyhow::{Context, Result, ensure};
use ara_gpu::{
    shared::{Frame, SharedSurface},
    vulkan::{Context as Gpu, external::ImportedSurface},
};

/// One embedded player's surface generation on the editor's device, and the frame
/// it shows. Close it with [`NativeUi::close_play_surface`].
pub struct PlaySurface {
    imported: ImportedSurface,
    shown: Option<Frame>,
    texture: Option<u32>,
}

impl NativeUi {
    /// Imports `surface` (from `Surfaces::surfaces`) on this adapter's device: three
    /// slots with sampled views. Fails with `ARA_GPU_EXTERNAL` when Play renders on
    /// another GPU or driver.
    /// # Safety
    /// `gpu` is the device this adapter was created on and outlives the surface;
    /// queue access stays on this thread.
    pub unsafe fn import_play_surface(
        &self,
        gpu: &Gpu,
        surface: &SharedSurface,
    ) -> Result<PlaySurface> {
        ensure!(
            gpu.device.handle() == self.memory.device.handle(),
            "import play surfaces on the adapter's own device"
        );
        // SAFETY: Forwarded caller contract.
        let imported = unsafe { ImportedSurface::new(gpu, surface)? };
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

    /// Unregisters an external image and parks its descriptor on the blank texture.
    /// Use between UI frames when a Scene target is unwritten or was replaced;
    /// the adapter must not retain a stale native image in its next submission.
    pub fn forget_external(&mut self, id: u32) {
        if let Some(slot) = self.external.remove(&id) {
            self.external_images.remove(&id);
            if let Some(view) = self.textures.get(&0).map(|t| t.image.view) {
                self.write_texture(slot, view);
            }
            self.free_slots.push(slot);
        }
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

    /// Shows `frame` (taken from `Surfaces`) as UI texture `id`: acquires its slot
    /// from the player, hands the previously shown slot back on the GPU and binds
    /// `id` to the new slot. Returns the previously shown frame: release it to its
    /// player. Call between UI frames (`NativeUi::end` waits for its submit, so the
    /// previous frame is no longer sampled).
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
        // SAFETY: The UI's last submit completed; the player published `frame`.
        unsafe {
            self.imported
                .exchange(Some(frame.slot), previous.map(|p| p.slot))?;
        }
        let (image, view) = self
            .imported
            .image(frame.slot)
            .context("frame slot outside the surface")?;
        if let Some(old) = self.texture.filter(|t| *t != id) {
            ui.forget_external(old);
        }
        // SAFETY: The slot image and view live on the adapter's device until this
        // surface is closed, which unregisters them first.
        unsafe { ui.external_image(id, image, view)? };
        self.texture = Some(id);
        self.shown = Some(frame);
        Ok(previous)
    }

    /// Stops showing: unregisters the texture and hands the shown slot back on the
    /// GPU. Returns the frame to release to its player. Call between UI frames.
    pub fn hide(&mut self, ui: &mut NativeUi) -> Result<Option<Frame>> {
        if let Some(id) = self.texture.take() {
            ui.forget_external(id);
        }
        let shown = self.shown.take();
        if let Some(frame) = shown {
            // SAFETY: The UI's last submit completed and no longer references it.
            unsafe { self.imported.exchange(None, Some(frame.slot))? };
        }
        Ok(shown)
    }

    /// The shown frame's pixels (screenshots, verification).
    pub fn read(&mut self) -> Result<image::RgbaImage> {
        let frame = self.shown.context("no frame shown")?;
        let [width, height] = self.size();
        // SAFETY: The shown slot is acquired; queue access stays on this thread.
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_gpu::{
        shared::Mailbox,
        vk,
        vulkan::{VulkanDevice, external::ExportRing, image_barrier},
    };
    use ara_ui_render::gpu::UiGpu;

    /// Clears an RGBA8 image to `rgba` and leaves it in TRANSFER_SRC_OPTIMAL.
    fn fill(gpu: &Gpu, image: vk::Image, rgba: [f32; 4], first: bool) {
        gpu.immediate(|command| {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let clear = (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            );
            let to_dst = image_barrier(
                image,
                vk::ImageAspectFlags::COLOR,
                if first {
                    vk::ImageLayout::UNDEFINED
                } else {
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL
                },
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                (
                    vk::PipelineStageFlags2::ALL_COMMANDS,
                    vk::AccessFlags2::MEMORY_READ,
                ),
                clear,
            );
            let to_src = image_barrier(
                image,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                clear,
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
            );
            // SAFETY: A fresh recording command buffer on this device; the image is live.
            unsafe {
                gpu.device.cmd_pipeline_barrier2(
                    command,
                    &vk::DependencyInfo::default().image_memory_barriers(&[to_dst]),
                );
                gpu.device.cmd_clear_color_image(
                    command,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: rgba },
                    &[range],
                );
                gpu.device.cmd_pipeline_barrier2(
                    command,
                    &vk::DependencyInfo::default().image_memory_barriers(&[to_src]),
                );
            }
        })
        .unwrap();
    }

    /// A player-side ring on one device, shown through the UI adapter of another:
    /// UI submits run their external-image barriers around it, every slot cycles,
    /// pixels survive and both devices stay validation-clean.
    #[test]
    #[ignore = "needs a Vulkan GPU with VK_KHR_external_memory_win32 and validation layers"]
    fn play_surfaces_show_through_the_external_image_path() {
        let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/test-gpu");
        let mut player = VulkanDevice::new(&[], &cache).unwrap();
        let editor = VulkanDevice::new(&[], &cache).unwrap();
        let size = [32, 16];
        let source = player
            .create_image(
                size[0],
                size[1],
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::SAMPLED,
            )
            .unwrap();
        let source = player.native_image(source).unwrap();
        // SAFETY: Both devices outlive the ring, adapter and surface; one thread.
        let mut ring = unsafe { ExportRing::new(player.context(), size).unwrap() };
        let shared = SharedSurface::duplicate_local(&ring.desc()).unwrap();
        // SAFETY: As above.
        let mut ui = unsafe { NativeUi::new(editor.context()).unwrap() };
        // SAFETY: As above.
        let mut surface = unsafe { ui.import_play_surface(editor.context(), &shared).unwrap() };
        let mut mailbox = Mailbox::default();
        for round in 0..6u8 {
            let red = 40 * round;
            fill(
                player.context(),
                source,
                [f32::from(red) / 255.0, 0.5, 0.0, 1.0],
                round == 0,
            );
            // SAFETY: `source` is the ring's size, RGBA8, in TRANSFER_SRC.
            let frame = unsafe { ring.publish(source, u64::from(round)).unwrap() }.unwrap();
            assert!(mailbox.deliver(frame).is_none());
            let frame = mailbox.take().unwrap();
            if let Some(previous) = surface.show(&mut ui, 7, frame).unwrap() {
                assert!(ring.release(previous));
            }
            ui.begin([size[0], size[1]]).unwrap();
            ui.end().unwrap();
            let pixel = surface.read().unwrap().get_pixel(3, 3).0;
            let expected = [red, 128, 0, 255];
            assert!(
                pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                "round {round}: {pixel:?} != {expected:?}"
            );
        }
        let last = ui.close_play_surface(surface).unwrap().unwrap();
        assert!(ring.release(last));
        ui.begin([size[0], size[1]]).unwrap();
        ui.end().unwrap();
        drop(ui);
        drop(ring);
        player.assert_clean().unwrap();
        editor.assert_clean().unwrap();
    }
}
