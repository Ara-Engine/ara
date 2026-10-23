#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Offline/editor ground-truth capture over Ara's shared transport semantics.
pub mod metrics;
pub mod robust_scenes;
pub mod scenes;
use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::{CameraSpec, Scene};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};

/// Progressive undenoised reference renderer. Independent samples accumulate in
/// linear radiance; a camera/content change requires an explicit reset.
pub struct Reference {
    view: FidelityView,
    target: u32,
}
impl Reference {
    /// Use the exact scene/material/light extraction used by the hybrid view.
    pub fn new(
        gpu: &mut VulkanDevice,
        scene: &Scene,
        size: [u32; 2],
        samples: u32,
        bounces: u32,
        post: PostSettings,
    ) -> Result<Self> {
        let settings = Settings {
            render_size: size,
            display_size: size,
            tier: Tier::Rt,
            mode: Mode::Reference { samples, bounces },
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 1.,
            post,
        };
        Ok(Self {
            view: FidelityView::new(gpu, scene, settings, None)?,
            target: samples,
        })
    }
    /// Render one bounded batch (at most eight samples/pixel) to avoid long kernels.
    pub fn step(
        &mut self,
        gpu: &mut VulkanDevice,
        camera: &CameraSpec,
        reset: bool,
    ) -> Result<bool> {
        self.view.render(
            gpu,
            &FrameInput {
                camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        Ok(self.view.reference_samples() >= self.target)
    }
    /// Converge to the configured finite budget without denoising the reference.
    pub fn converge(&mut self, gpu: &mut VulkanDevice, camera: &CameraSpec) -> Result<()> {
        while !self.step(gpu, camera, false)? {}
        Ok(())
    }
    /// Shared display transform applied to the undenoised reference.
    pub fn capture(&self, gpu: &mut VulkanDevice) -> Result<image::RgbaImage> {
        self.view.capture(gpu)
    }
    /// Accumulated linear radiance before any post processing.
    pub fn radiance(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        self.view.capture_radiance(gpu)
    }
    /// Address-independent fingerprint of the shared scene/material/light extraction.
    pub fn scene_hash(&self) -> &str {
        self.view.scene_hash()
    }
    /// Release resources before the device owner is dropped.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        self.view.destroy(gpu)
    }
}
