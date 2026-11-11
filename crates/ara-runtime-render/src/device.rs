//! The device the presenter and the editor's scene compositor share.
use anyhow::Result;
use ara_gpu::GpuDevice;
use std::path::PathBuf;

/// The GPU owner for presentation.
///
/// The device is boxed, so its address stays put when this value moves between
/// threads. A renderer built with `Renderer::on_device` borrows it and must be
/// dropped first. On Apple `open` is `GpuDevice::new`, the same constructor the
/// bootstrap renderer used, which is the Metal device.
pub struct PresentationDevice {
    gpu: Box<GpuDevice>,
}

impl PresentationDevice {
    /// Opens the presentation device: hardware ray queries where the GPU has
    /// them, the same device `Renderer::new_hybrid` created. `extensions` are
    /// the window's (empty when headless). `cache` holds the pipeline cache.
    pub fn open(extensions: &[std::ffi::CString], cache: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            gpu: Box::new(GpuDevice::new(extensions, cache.into())?),
        })
    }

    /// Adopts a device the caller already opened (a loading handoff).
    pub fn from_device(gpu: GpuDevice) -> Self {
        Self { gpu: Box::new(gpu) }
    }

    /// The device external renderers and the editor compositor record on.
    pub fn gpu(&self) -> &GpuDevice {
        &self.gpu
    }

    /// The device, mutably, between completed frames.
    pub fn gpu_mut(&mut self) -> &mut GpuDevice {
        &mut self.gpu
    }
}
