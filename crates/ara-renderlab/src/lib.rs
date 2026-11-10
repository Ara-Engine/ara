//! Renderlab as an Ara renderer: [`Renderlab`] implements
//! `ara_runtime_render::ExternalRenderer`, so a host installs it with
//! `Presenter::set_external_renderer` and every present and capture draws the
//! extracted frame through Renderlab's `Engine`.
//!
//! This crate is the only place that names a Renderlab (or wgpu) type. Renderlab
//! owns its own device today (D35); its image reaches the presenter's device through
//! images the two devices share where they can, and by a CPU copy otherwise. Every
//! frame reports the crossing's cost (`ExternalFrame::handoff_ms`). When Renderlab's
//! passes move onto `ara-gpu`, that change stays inside this crate.
//!
//! What Ara renders that this path does not yet is in `AGENTS.md`; each such
//! feature reports `ARA_RENDERLAB_UNSUPPORTED` once when a frame uses it.
#![warn(missing_docs)]

#[cfg(not(target_vendor = "apple"))]
mod decals;
#[cfg(not(target_vendor = "apple"))]
mod environment;
#[cfg(not(target_vendor = "apple"))]
mod gaps;

#[cfg(not(target_vendor = "apple"))]
mod bridge;
#[cfg(not(target_vendor = "apple"))]
mod lighting;
#[cfg(not(target_vendor = "apple"))]
mod scene;
#[cfg(not(target_vendor = "apple"))]
mod shared;
#[cfg(not(target_vendor = "apple"))]
mod terrain;
#[cfg(not(target_vendor = "apple"))]
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
    /// default (0.5). Its glossy band (above 0.05) can leave bright speckles on glossy
    /// surfaces at v0.1.2; 0.05 traces mirrors only.
    pub max_reflection_roughness: Option<f32>,
    /// Hand frames to the presenter by CPU copy even where the two devices could
    /// share images (A/B measurements; the automatic fallback needs no flag).
    pub cpu_copy_handoff: bool,
}

/// Renderlab behind Ara's presenter.
pub struct Renderlab {
    #[cfg(not(target_vendor = "apple"))]
    bridge: bridge::Bridge,
    #[cfg(target_vendor = "apple")]
    never: std::convert::Infallible,
}

impl Renderlab {
    /// Creates Renderlab's engine on its own device. Fails with `ARA_RENDERLAB_UNAVAILABLE` when this
    /// machine cannot run the requested path; the host then keeps Ara's renderer.
    pub fn new(options: &Options) -> Result<Self> {
        #[cfg(not(target_vendor = "apple"))]
        {
            Ok(Self {
                bridge: bridge::Bridge::new(options)?,
            })
        }
        #[cfg(target_vendor = "apple")]
        {
            let _ = options;
            anyhow::bail!(
                "ARA_RENDERLAB_UNAVAILABLE: Renderlab has no Metal path yet (it renders through Vulkan only)"
            )
        }
    }

    /// The path in use.
    pub fn path(&self) -> ActivePath {
        #[cfg(not(target_vendor = "apple"))]
        {
            self.bridge.path()
        }
        #[cfg(target_vendor = "apple")]
        match self.never {}
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
        #[cfg(not(target_vendor = "apple"))]
        {
            self.bridge.render(gpu, view)
        }
        #[cfg(target_vendor = "apple")]
        {
            let _ = (gpu, view);
            match self.never {}
        }
    }

    fn cut(&mut self) {
        #[cfg(not(target_vendor = "apple"))]
        self.bridge.cut();
    }

    fn settle_frames(&self) -> u32 {
        // Temporal reconstruction, the GI cache and metering converge over about
        // half a second of frames (Renderlab's own example renders 48).
        48
    }

    fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        #[cfg(not(target_vendor = "apple"))]
        {
            self.bridge.release(gpu)
        }
        #[cfg(target_vendor = "apple")]
        {
            let _ = gpu;
            Ok(())
        }
    }
}
