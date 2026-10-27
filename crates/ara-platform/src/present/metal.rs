//! macOS presentation: an SDL Metal view whose CAMetalLayer is presented through
//! `ara_gpu::metal::present::Presenter`, with the backend-neutral surface of the
//! Vulkan `Present`. Engine output is scaled and converted to the BGRA8 drawable, as
//! the Vulkan blit does. HDR (EDR) output is not implemented yet, so only SDR output
//! settings are accepted; the drawable tracks the window size at creation, and callers
//! recreate presentation when the window resizes, as they do on Vulkan.
use crate::window::{Window, error};
use anyhow::{Context, Result, bail, ensure};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::metal::present::Presenter;
use ara_gpu::{GpuDevice, Image, vk};
use sdl3_sys::metal::{
    SDL_Metal_CreateView, SDL_Metal_DestroyView, SDL_Metal_GetLayer, SDL_MetalView,
};
use std::ptr::NonNull;

/// An SDL Metal view and the presenter drawing into its layer.
pub struct Present {
    /// None once retired; dropped before the view that owns its layer.
    presenter: Option<Presenter>,
    view: SDL_MetalView,
    output: OutputSettings,
    format: vk::SurfaceFormatKHR,
}

impl Present {
    /// The Vulkan presentation's opt-in overlap (no queue idle after each blit);
    /// every Metal `present_image` still waits its own work, so it has no effect.
    /// # Safety
    /// `gpu` created this presentation and is live.
    pub unsafe fn set_frames_in_flight(&mut self, _gpu: &GpuDevice, _enabled: bool) -> Result<()> {
        Ok(())
    }
    /// Present into `window` with SDR output.
    /// # Safety
    /// Keep the device and window alive until [`Self::retire`] or drop; call from the
    /// window's (main) thread.
    pub unsafe fn create(gpu: &GpuDevice, window: &Window) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        unsafe { Self::create_with_output(gpu, window, OutputSettings::default()) }
    }
    /// Present with explicit output settings; HDR is rejected until EDR lands.
    /// # Safety
    /// As [`Self::create`].
    pub unsafe fn create_with_output(
        gpu: &GpuDevice,
        window: &Window,
        output: OutputSettings,
    ) -> Result<Self> {
        ensure!(output.valid(), "Invalid HDR paper-white/peak luminance");
        ensure!(
            output.mode == OutputMode::Sdr,
            "HDR output is not available on the Metal backend yet"
        );
        let size = window.size()?;
        // SAFETY: the caller keeps the window alive on its thread; SDL owns the view
        // until `SDL_Metal_DestroyView` in `Drop`.
        let view = unsafe { SDL_Metal_CreateView(window.handle) };
        ensure!(!view.is_null(), "SDL Metal view: {}", error());
        let mut present = Self {
            presenter: None,
            view,
            output,
            format: vk::SurfaceFormatKHR::default(),
        };
        // SAFETY: `view` is live until `Drop`.
        let Some(layer) = NonNull::new(unsafe { SDL_Metal_GetLayer(view) }) else {
            bail!("SDL Metal layer: {}", error());
        };
        // SAFETY: the view's CAMetalLayer outlives the presenter, which `Drop` releases
        // first, and only this presenter presents into it.
        let presenter = unsafe { Presenter::from_layer(gpu, layer, size) }?;
        present.format = presenter.surface_format();
        present.presenter = Some(presenter);
        Ok(present)
    }
    /// Present a completed engine image, scaled to the drawable.
    /// # Safety
    /// `image` is a completed transfer source on `gpu`; both owners are live.
    pub unsafe fn present_image(
        &mut self,
        gpu: &GpuDevice,
        image: Image,
        _size: [u32; 2],
    ) -> Result<()> {
        self.presenter
            .as_mut()
            .context("presentation was retired")?
            .display(gpu, image)?;
        Ok(())
    }
    /// Present an image encoded for the selected output; only SDR exists on Metal.
    /// # Safety
    /// As [`Self::present_image`].
    pub unsafe fn present_encoded(
        &mut self,
        gpu: &GpuDevice,
        image: Image,
        size: [u32; 2],
        encoding: OutputMode,
    ) -> Result<()> {
        ensure!(
            encoding == self.output.mode,
            "Presentation/output encoding mismatch"
        );
        // SAFETY: forwarded caller contract.
        unsafe { self.present_image(gpu, image, size) }
    }
    /// Release the presenter and view. Every `present_image` waited for its own work,
    /// so nothing is in flight; callers that need a device-wide idle wait first.
    /// # Safety
    /// Created on `gpu`; not presented with afterwards.
    pub unsafe fn retire(&mut self, _gpu: &GpuDevice) {
        self.release();
    }
    /// Active display transfer and paper-white policy (SDR on Metal).
    pub fn output_settings(&self) -> OutputSettings {
        self.output
    }
    /// The drawable's format and color space, described in Vulkan terms.
    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        self.format
    }
    fn release(&mut self) {
        self.presenter = None;
        if !self.view.is_null() {
            // SAFETY: the view was created by `create_with_output` and is destroyed
            // once, after the presenter that used its layer.
            unsafe { SDL_Metal_DestroyView(self.view) };
            self.view = std::ptr::null_mut();
        }
    }
}

impl Drop for Present {
    fn drop(&mut self) {
        self.release();
    }
}
