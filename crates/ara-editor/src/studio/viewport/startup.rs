//! The scene device is opened on a worker; the renderer borrows it. The UI
//! adopts both. Drop the renderer before the device (`let (device, renderer)`).
use super::*;
use std::{ffi::CString, thread::JoinHandle};

/// Keep pumping the loading window/authority while cold PSOs and initial scene
/// resources are created. No Vulkan owner is shared across these threads.
///
/// The device is the first element so a `let (device, renderer)` binding drops
/// the renderer first.
pub(crate) fn prepare_renderer(
    scene: ara_frame::scene::Scene,
    size: [u32; 2],
    extensions: Vec<CString>,
    cache: PathBuf,
    ray_tracing: bool,
    quality: ara_runtime_render::QualityChoice,
    tick: impl FnMut(Duration) -> ara_ui::Result<bool>,
) -> ara_ui::Result<Option<(ara_runtime_render::PresentationDevice, Renderer)>> {
    let job = std::thread::Builder::new()
        .name("ara-scene-startup".into())
        .spawn(move || {
            ara_gpu::cpu_scope!("SceneView.renderer_startup_worker");
            // The same device `Renderer::new` / `new_hybrid` opened: hardware ray
            // queries where the GPU has them. Presets trace none without the opt-in.
            // On Apple this is the Metal device.
            if ray_tracing {
                anyhow::ensure!(!scene.meshes.is_empty(), "Scene has no geometry");
            }
            let mut device = ara_runtime_render::PresentationDevice::open(&extensions, cache)?;
            let ptr = device.gpu_mut() as *mut ara_gpu::GpuDevice;
            // SAFETY: `device` boxes the GpuDevice, so `ptr` stays valid when the
            // pair moves to the UI thread. The caller drops the renderer first.
            let mut renderer = unsafe { Renderer::on_device(scene, size, ptr)? };
            if !ray_tracing {
                let plan = Plan::resolve(
                    quality,
                    &Capabilities::of(renderer.gpu()),
                    [1, 1],
                    ara_runtime_render::RenderOptions { ray_tracing: false },
                )?;
                if let RenderPath::Raster(plan) = plan.path {
                    renderer.prewarm_view_quality(plan.quality)?;
                }
            }
            Ok((device, renderer))
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
            .map_err(|_| "scene renderer preparation panicked")??,
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
