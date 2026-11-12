//! The GPU device the Scene view's Renderlab image lives on, and the size it draws at. It is
//! all the old renderer was to the editor once Renderlab drew the Scene: the compositor, the
//! Game layer, the graph pages and the captures borrow its `GpuDevice`.
use ara_gpu::GpuDevice;
use ara_runtime_render::PresentationDevice;
use std::{
    ffi::CString,
    path::PathBuf,
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The editor's GPU device and the Scene view's output size in physical pixels.
pub(crate) struct SceneDevice {
    device: PresentationDevice,
    size: [u32; 2],
}

impl SceneDevice {
    /// Creates the device at `size` on this thread (the windowed studio uses [`prepare`]).
    pub(crate) fn new(
        extensions: &[CString],
        cache: PathBuf,
        size: [u32; 2],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            device: PresentationDevice::open(extensions, cache)?,
            size,
        })
    }

    /// The device (shared by every GPU user of the studio) and the current output size.
    pub(crate) fn gpu(&self) -> &GpuDevice {
        self.device.gpu()
    }

    pub(crate) fn gpu_mut(&mut self) -> &mut GpuDevice {
        self.device.gpu_mut()
    }

    /// The Scene view's output size.
    pub(crate) fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Follows the Scene view's layer: Renderlab draws at this size from the next pass.
    pub(crate) fn resize(&mut self, size: [u32; 2]) {
        self.size = size;
    }
}

/// Creates the device on a worker while the loading window and the authority keep pumping
/// (`tick`, which says whether to go on): no GPU owner is shared across these threads. `None`
/// when `tick` asked to stop.
pub(crate) fn prepare(
    extensions: Vec<CString>,
    cache: PathBuf,
    size: [u32; 2],
    tick: impl FnMut(Duration) -> ara_ui::Result<bool>,
) -> ara_ui::Result<Option<SceneDevice>> {
    let job = std::thread::Builder::new()
        .name("ara-scene-startup".into())
        .spawn(move || {
            ara_gpu::cpu_scope!("SceneView.device_startup_worker");
            SceneDevice::new(&extensions, cache, size)
        })?;
    wait_for(job, tick)
}

fn wait_for<T>(
    job: JoinHandle<anyhow::Result<T>>,
    mut tick: impl FnMut(Duration) -> ara_ui::Result<bool>,
) -> ara_ui::Result<Option<T>> {
    let began = Instant::now();
    while !job.is_finished() {
        if !tick(began.elapsed())? {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    Ok(Some(
        job.join()
            .map_err(|_| "the GPU device startup panicked")??,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_construction_keeps_pumping_the_owner_until_adoption() -> ara_ui::Result<()> {
        let owner = std::thread::current().id();
        let (release, gate) = std::sync::mpsc::channel();
        let job = std::thread::spawn(move || {
            gate.recv()?;
            assert_ne!(owner, std::thread::current().id());
            Ok(7)
        });
        let mut ticks = 0;
        let result = wait_for(job, |_| {
            assert_eq!(owner, std::thread::current().id());
            ticks += 1;
            if ticks == 3 {
                release.send(())?;
            }
            Ok(true)
        })?;
        assert_eq!(result, Some(7));
        assert!(ticks >= 3);
        Ok(())
    }
}
