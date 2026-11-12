//! The device the presenter and the editor's scene compositor share.
use anyhow::Result;
use ara_gpu::{GpuDevice, GpuLease};
use std::path::PathBuf;

/// The GPU owner for presentation.
///
/// The device sits in a [`GpuLease`], so its address stays put when this value moves.
/// On Apple `open` is `GpuDevice::new`, which is the Metal device.
pub struct PresentationDevice {
    lease: GpuLease,
}

impl PresentationDevice {
    /// Opens the presentation device: hardware ray queries where the GPU has them.
    /// `extensions` are the window's (empty when headless). `cache` holds the pipeline cache.
    pub fn open(extensions: &[std::ffi::CString], cache: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            lease: GpuLease::new(GpuDevice::new(extensions, cache.into())?),
        })
    }

    /// Adopts a device the caller already opened (a loading handoff).
    pub fn from_device(gpu: GpuDevice) -> Self {
        Self {
            lease: GpuLease::new(gpu),
        }
    }

    /// The device external renderers and the editor compositor record on.
    pub fn gpu(&self) -> &GpuDevice {
        self.lease.get()
    }

    /// The device, mutably, between completed frames.
    pub fn gpu_mut(&mut self) -> &mut GpuDevice {
        // SAFETY: `&mut self` is the recording thread's use of this owner between
        // frames, and no other lease on the allocation exists.
        unsafe { &mut *self.lease.as_mut_ptr() }
    }
}
