//! Shaderless main-thread presentation while runtime and presenter workers prepare.
use super::*;
use ara_runtime_render::{QualityChoice, RenderOptions};
use std::{collections::BTreeMap, path::Path, thread, time::Duration};
mod graphics;
#[cfg(test)]
mod tests;

pub(super) struct Settings<'a> {
    pub name: &'a str,
    pub size: [u32; 2],
    pub cache: &'a Path,
    pub quality: QualityChoice,
    pub options: RenderOptions,
    pub hidden: bool,
}

pub(super) struct Loaded {
    pub world: World,
    pub build_ms: f64,
    pub window: device::Windowed,
    pub presenter: Presenter,
}

/// SDL and the loading device stay on main. The full presenter and its pipelines
/// move here only after warming; no GPU owner is concurrently accessed. Closing
/// the loading window drops worker handles without waiting on import/compilation.
pub(super) fn load(
    scene: Arc<ara_play::PlayScene>,
    source: Arc<dyn ara_runtime::assets::AssetSource>,
    settings: Settings<'_>,
    began: Instant,
    phases: &mut BTreeMap<&'static str, u128>,
) -> Result<Option<Loaded>> {
    let mut world_job = Some(
        thread::Builder::new()
            .name("ara-player-startup".into())
            .spawn(move || World::prepare_packed(&scene, source))?,
    );
    let (mut window, extensions, size) =
        device::Windowed::open_unprepared(settings.name, settings.size, settings.hidden)?;
    let loading_started = Instant::now();
    let mut loading = device::Loading::new(&extensions, settings.cache)?;
    phases.insert("loading_device_ms", loading_started.elapsed().as_millis());
    phases.insert("device_ready_ms", began.elapsed().as_millis());
    let mut capture = std::env::var_os("ARA_LOADING_CAPTURE").map(PathBuf::from);
    let mut graphics = None;
    let mut graphics_inputs = Some(extensions);
    let mut ready_presenter = None;
    let mut prepared = None;
    let mut shown_phase = "";
    loop {
        let frame_start = Instant::now();
        if window.events(ara_input::PlayerId(0))?.0 {
            return Ok(None);
        }
        if world_job.as_ref().is_some_and(|job| job.is_finished()) {
            prepared = Some(
                world_job
                    .take()
                    .context("runtime worker")?
                    .join()
                    .map_err(|_| anyhow::anyhow!("shipping runtime preparation panicked"))??,
            );
            phases.insert("runtime_ready_ms", began.elapsed().as_millis());
        }
        if graphics
            .as_ref()
            .is_some_and(graphics::Graphics::is_finished)
        {
            let (presenter, elapsed) = graphics.take().context("graphics worker")?.finish()?;
            phases.insert("presenter_warmup_ms", elapsed.as_millis());
            phases.insert("presenter_ready_ms", began.elapsed().as_millis());
            ready_presenter = Some(presenter);
        }
        let phase = if prepared.is_some() {
            "Preparing renderer"
        } else {
            "Loading scene"
        };
        if phase != shown_phase {
            window.title(&format!("{} — {phase}", settings.name))?;
            shown_phase = phase;
        }
        let completed = u8::from(prepared.is_some()) + u8::from(ready_presenter.is_some());
        if window.present_loading(&mut loading, 0.1 + f32::from(completed) * 0.4)? {
            phases
                .entry("loading_frame_ms")
                .or_insert_with(|| began.elapsed().as_millis());
            phases
                .entry("first_frame_ms")
                .or_insert_with(|| began.elapsed().as_millis());
            if let Some(path) = capture.take() {
                loading.capture(&path, size)?;
            }
            // Start compilation only after a real frame has reached the window.
            // The main-thread first presentation can never wait behind it.
            if let Some(extensions) = graphics_inputs.take() {
                graphics = Some(graphics::Graphics::start(
                    size,
                    extensions,
                    settings.cache.to_path_buf(),
                    settings.quality,
                    settings.options,
                )?);
            }
        }
        if prepared.is_some() && ready_presenter.is_some() {
            let (prepared, build_ms) = prepared.take().context("prepared runtime")?;
            let world = prepared.map_runtime(|runtime| Ok(crate::net::Play::Local(runtime)))?;
            let mut presenter = ready_presenter.take().context("warmed presenter")?;
            presenter.set_probe_gi(true);
            presenter.set_quality(settings.quality);
            // Drop the temporary swapchain/device before the game presenter
            // creates its surface. The same SDL window remains alive throughout.
            drop(loading);
            window.title(settings.name)?;
            window.begin_game();
            return Ok(Some(Loaded {
                world,
                build_ms,
                window,
                presenter,
            }));
        }
        if let Some(wait) = Duration::from_millis(16).checked_sub(frame_start.elapsed()) {
            thread::sleep(wait.min(Duration::from_millis(8)));
        }
    }
}
