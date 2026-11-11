//! Renderlab as an Ara renderer: [`Renderlab`] implements
//! `ara_runtime_render::ExternalRenderer`, so a host installs it with
//! `Presenter::set_external_renderer` and every present and capture draws the
//! extracted frame through Renderlab's `Engine`.
//!
//! This crate is the only place that names a Renderlab (or wgpu) type. Where Renderlab
//! opens Vulkan through wgpu (`bridge`) it owns its own device (D35), and its image
//! reaches the presenter's device through images the two devices share where they
//! can, and by a CPU copy otherwise; every frame reports the crossing's cost
//! (`ExternalFrame::handoff_ms`). On Apple (`bridge_metal`) Renderlab's ara-gpu backend
//! renders on the presenter's own device, so nothing crosses. Both map the extracted
//! frame with the same modules, through `rl::Engine`.
//!
//! What Ara renders that this path does not yet is in `AGENTS.md`; each such
//! feature reports `ARA_RENDERLAB_UNSUPPORTED` once when a frame uses it.
#![warn(missing_docs)]

// On Apple, Renderlab's ara-gpu backend is a separate revision (Cargo.toml); it is
// `renderlab` to every module, as the wgpu revision is elsewhere.
#[cfg(target_vendor = "apple")]
extern crate renderlab_metal as renderlab;

mod atmosphere;
mod batch;
mod cutout;
mod decals;
mod effects;
// On Apple the backend has no environment maps yet: only `mean_radiance` is used there.
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
mod environment;
mod gaps;
mod rl;

#[cfg(not(target_vendor = "apple"))]
mod bridge;
#[cfg(target_vendor = "apple")]
mod bridge_metal;
mod grade;
mod lighting;
mod meter;
mod scene;
#[cfg(not(target_vendor = "apple"))]
mod shared;
mod splat;
mod terrain;
mod texture;

use anyhow::Result;
use ara_gpu::GpuDevice;
use ara_runtime_render::{ExternalFrame, ExternalRenderer, ExternalView};
use std::path::PathBuf;

/// Which Renderlab path to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathChoice {
    /// Hardware ray tracing where the GPU has it, else the non-RT path.
    #[default]
    Auto,
    /// Hardware ray queries (traced primaries, sun visibility and GI).
    HardwareRt,
    /// No ray-tracing hardware needed. Its sun-shadow method carries an open patent
    /// question (`ARA_RENDERLAB_NONRT_PATENT`): prefer `HardwareRt` where the GPU supports it.
    NonRt,
}

/// The Renderlab path that draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePath {
    /// Hardware ray queries.
    HardwareRt,
    /// The visibility-buffer path without ray-tracing hardware.
    NonRt,
}

/// How to create [`Renderlab`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The path to request.
    pub path: PathChoice,
    /// Directory for Renderlab's persistent pipeline cache (kept apart from Ara's).
    pub pipeline_cache: Option<PathBuf>,
    /// Roughness at which Renderlab stops tracing reflections; `None` keeps its own
    /// default (0.5); 0.05 traces mirrors only (an A/B switch for the glossy band's cost).
    pub max_reflection_roughness: Option<f32>,
    /// Hand frames to the presenter by CPU copy even where the two devices could
    /// share images (A/B measurements; the automatic fallback needs no flag).
    pub cpu_copy_handoff: bool,
    /// Light the scene with a constant sky of the sky image's mean radiance instead of
    /// the image itself. For A/B measurement of what a sky map costs in Renderlab.
    pub constant_sky: bool,
}

/// Renderlab behind Ara's presenter.
pub struct Renderlab {
    #[cfg(not(target_vendor = "apple"))]
    bridge: bridge::Bridge,
    #[cfg(target_vendor = "apple")]
    bridge: bridge_metal::Bridge,
}

impl Renderlab {
    /// Creates Renderlab's engine: on its own Vulkan device through wgpu, or on Apple
    /// on the presenter's Metal device through Renderlab's ara-gpu backend. Fails with
    /// `ARA_RENDERLAB_UNAVAILABLE` when this machine cannot run the requested path; the
    /// host then keeps Ara's renderer.
    pub fn new(options: &Options) -> Result<Self> {
        #[cfg(not(target_vendor = "apple"))]
        let bridge = bridge::Bridge::new(options)?;
        #[cfg(target_vendor = "apple")]
        let bridge = bridge_metal::Bridge::new(options)?;
        Ok(Self { bridge })
    }

    /// The path in use.
    pub fn path(&self) -> ActivePath {
        self.bridge.path()
    }
}

impl ExternalRenderer for Renderlab {
    fn name(&self) -> &str {
        match self.path() {
            ActivePath::HardwareRt => "renderlab-rt",
            ActivePath::NonRt => "renderlab-nonrt",
        }
    }

    fn render(&mut self, gpu: &mut GpuDevice, view: &ExternalView<'_>) -> Result<ExternalFrame> {
        self.bridge.render(gpu, view)
    }

    fn cut(&mut self) {
        self.bridge.cut();
    }

    fn settle_frames(&self) -> u32 {
        // Temporal reconstruction, the GI cache and metering converge over about
        // half a second of frames (Renderlab's own example renders 48).
        48
    }

    fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        self.bridge.release(gpu)
    }
}
