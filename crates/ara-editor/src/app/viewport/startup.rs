//! Renderer construction owns its device on a worker until the UI adopts it.
use super::*;
use std::{ffi::CString, thread::JoinHandle};

/// Keep pumping the loading window/authority while cold PSOs and initial scene
/// resources are created. No Vulkan owner is shared across these threads.
pub(in crate::app) fn prepare_renderer(
    scene: ara_render::scene::Scene,
    size: [u32; 2],
    extensions: Vec<CString>,
    cache: PathBuf,
    ray_tracing: bool,
    tick: impl FnMut(Duration) -> ara_ui::Result<bool>,
) -> ara_ui::Result<Option<Renderer>> {
    let job = std::thread::Builder::new()
        .name("ara-scene-startup".into())
        .spawn(move || {
            ara_gpu::cpu_scope!("SceneView.renderer_startup_worker");
            let create = if ray_tracing {
                Renderer::new
            } else {
                Renderer::new_raster
            };
            create(scene, size, &extensions, cache)
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
