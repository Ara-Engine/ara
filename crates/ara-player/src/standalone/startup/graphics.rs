//! The full game presenter is created and warmed on one temporary owner thread,
//! then moved to the frame owner only after all its work has completed.
use super::*;
use std::thread::JoinHandle;

pub(super) struct Graphics {
    thread: JoinHandle<Result<(Presenter, Duration)>>,
}
impl Graphics {
    pub fn start(
        size: [u32; 2],
        gpu: ara_gpu::vulkan::VulkanDevice,
        cache: PathBuf,
        quality: QualityChoice,
        options: RenderOptions,
    ) -> Result<Self> {
        Self::spawn(move || {
            ara_gpu::cpu_scope!("Player.presenter_warmup_worker");
            let mut presenter = Presenter::with_device(size, cache, gpu);
            presenter.set_probe_gi(false);
            presenter.set_render_options(options);
            presenter.set_quality(quality);
            presenter.prepare(&device::warm_frame())?;
            Ok(presenter)
        })
    }

    pub(super) fn spawn(
        prepare: impl FnOnce() -> Result<Presenter> + Send + 'static,
    ) -> Result<Self> {
        Ok(Self {
            thread: thread::Builder::new()
                .name("ara-presenter-startup".into())
                .spawn(move || {
                    let began = Instant::now();
                    Ok((prepare()?, began.elapsed()))
                })?,
        })
    }
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
    pub fn finish(self) -> Result<(Presenter, Duration)> {
        self.thread
            .join()
            .map_err(|_| anyhow::anyhow!("presenter warm-up worker panicked"))?
    }
}
