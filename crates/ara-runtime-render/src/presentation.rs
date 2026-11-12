//! One presentation path for windows, embedded surfaces and explicit readbacks: the
//! external renderer draws, the presenter publishes its completed image.
use super::*;
use ara_gpu::{Access, Barrier, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk};
use ara_render_fidelity::presets::Plan;
pub use ara_render_fidelity::presets::{QualityChoice, QualityPreset, RenderOptions};
use ara_render_fidelity::environment::FrameEnvironment;
use fidelity::native::Content;

/// A completed display image independent of the renderer's working targets.
pub struct PresentationImage {
    pub(super) image: Image,
    size: [u32; 2],
}

mod capture;
mod published;

impl Presenter {
    /// How long hosts wait for an earlier GPU frame (a UI composite, a window copy)
    /// before they skip: far beyond a healthy frame, short enough that a stalled present
    /// path (minimized, occluded or undisplayed window) never stops the window responding.
    pub const STALL_WAIT: Duration = Duration::from_millis(100);

    /// Engine-private correlation for the next presentation submission. Does not
    /// change rendering, synchronization or simulation; None leaves it unsampled.
    pub fn profile_frame(&mut self, frame: Option<u64>) {
        self.profile_next = frame;
    }

    /// Takes only completed pass samples with their original submitting frame ID.
    /// Delayed samples never inherit the CPU frame in which readback was collected.
    pub fn take_profile_timings(&mut self) -> Option<(u64, Vec<ara_gpu::commands::PassTiming>)> {
        self.profile_completed.pop_front()
    }

    /// The last frame's GPU timings as the external renderer reported them: its whole
    /// frame, then each named pass (`ExternalFrame::passes`).
    pub fn pass_timings(&self) -> &[ara_gpu::vulkan::PassTiming] {
        &self.timings
    }

    /// Development shader provenance for the presentation device.
    pub fn shader_status(&self) -> ara_gpu::vulkan::ShaderStatus {
        self.gpu()
            .map(ara_gpu::GpuDevice::shader_status)
            .unwrap_or_default()
    }

    /// Queue a shader replacement on the presentation device without blocking this frame.
    pub fn reload_shaders(&mut self) {
        if let Some(gpu) = self.gpu_mut() {
            gpu.request_shader_reload();
        }
    }

    /// Selects a user override, or the project's setting when no override exists.
    pub fn set_quality(&mut self, quality: QualityChoice) {
        self.choice = quality;
        if let Some(settings) = &mut self.graphics {
            use ara_graphics_settings::Preset;
            settings.preset = match quality {
                QualityChoice::Auto => Preset::Auto,
                QualityChoice::Preset(QualityPreset::Low) => Preset::Low,
                QualityChoice::Preset(QualityPreset::Medium) => Preset::Medium,
                QualityChoice::Preset(QualityPreset::High) => Preset::High,
                QualityChoice::Preset(QualityPreset::Epic) => Preset::Epic,
            };
        }
        self.preset = None;
    }

    /// Replaces the frame's authored look (look development and scoring at a fixed
    /// exposure); `None` returns to the authored look.
    pub fn set_look_override(&mut self, look: Option<FrameEnvironment>) {
        self.look_override = look;
    }

    /// Turns effects off for diagnosis (the editor's View > Rendering debug, sent with
    /// each Play). The exposure lock and the debug view apply to every frame; an
    /// external renderer is told about the other switches (`ExternalView::debug_switches`).
    pub fn set_debug(&mut self, debug: RenderDebug) {
        self.debug = debug;
    }

    /// The diagnosis switches in force, including the debug view captures draw.
    pub fn debug(&self) -> RenderDebug {
        self.debug
    }

    /// Renderer selection: the standard tiers, or the opt-in ray-traced tiers.
    pub fn set_render_options(&mut self, options: RenderOptions) {
        self.options = options;
        self.preset = None;
    }

    /// Preset the device's capabilities resolve the quality choice to, once the device is open.
    pub fn quality(&self) -> Option<QualityPreset> {
        self.preset
    }

    /// A failed present's diagnostic. The last completed display remains valid.
    pub fn presentation_error(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Requested display size.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Extent of `output()`, including a last-good frame retained across a failed resize.
    pub fn output_size(&self) -> [u32; 2] {
        self.published
            .as_ref()
            .map_or(self.size, PresentationImage::size)
    }

    /// Opens the device external renderers draw on, once.
    pub(crate) fn ensure_device(&mut self) -> Result<()> {
        if self.device.is_none() {
            // One device with hardware ray queries where the GPU has them. On Apple
            // this is the Metal device.
            self.device = Some(if let Some(gpu) = self.initial_device.take() {
                PresentationDevice::from_device(gpu)
            } else {
                PresentationDevice::open(&self.extensions, &self.cache)?
            });
        }
        if self.preset.is_none() {
            let caps =
                ara_render_fidelity::presets::Capabilities::of(self.gpu().context("device")?);
            self.preset =
                Some(Plan::resolve(self.choice, &caps, self.size, self.options)?.preset);
        }
        Ok(())
    }

    /// Opens the device and checks a renderer is installed. Resolving content happens in
    /// the first present, where the renderer needs it.
    pub fn prepare_with(
        &mut self,
        _frame: &RenderFrame,
        _resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Presenter.prepare");
        self.stats = PresentStats::default();
        self.ensure_device()
    }

    /// `prepare_with` for a frame with no resolved content.
    pub fn prepare(&mut self, frame: &RenderFrame) -> Result<()> {
        self.prepare_with(frame, || Ok(Content::default()))
    }

    /// Draws `frame` with the installed renderer and publishes its completed image.
    /// `resolve` runs when the draw count changes, for the environment map, the foliage
    /// parts and the terrain layer materials. A failed present keeps the last published
    /// image and reports the error through `presentation_error`.
    pub fn present_with(
        &mut self,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Presenter.present");
        self.stats.skipped = false;
        let result = self.present_external(frame, resolve);
        match result {
            Ok(()) => {
                self.failure = None;
                Ok(())
            }
            Err(error) => {
                self.stats.gpu_span_ms = None;
                self.stats.gpu_submission_gap_ms = None;
                if self.failure.is_none() {
                    eprintln!(
                        "ARA_GPU_BACKEND: presentation retains last completed frame: {error:#}"
                    );
                    self.failure = Some(format!("{error:#}"));
                }
                if self.published.is_some() {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Presents model-only content; resolved terrain/prefabs use `present_with`.
    pub fn present(&mut self, frame: &RenderFrame) -> Result<()> {
        self.present_with(frame, || Ok(Content::default()))
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let external = self.external.take();
        let image = self.published.take();
        if let Some(device) = self.device.as_mut() {
            let gpu = device.gpu_mut();
            if let Some(mut external) = external {
                let _ = external.release(gpu);
            }
            if let Some(image) = image {
                let _ = gpu.retire(Resource::Image(image.image));
            }
            let _ = gpu.collect();
        }
    }
}
