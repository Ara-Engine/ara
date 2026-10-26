//! Shaderless main-thread presentation while runtime and presenter workers prepare.
use super::*;
use ara_runtime_render::{QualityChoice, RenderOptions};
use std::{collections::BTreeMap, path::Path, thread, time::Duration};
mod content;
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

/// SDL and the loading recording owner stay on main. The game owner warms on a
/// separate queue (serialized on single-queue adapters), then takes over the same
/// swapchain. Closing detaches workers without waiting on import/compilation.
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
            .spawn(move || content::prepare(&scene, source))?,
    );
    let (mut window, extensions, size) =
        device::Windowed::open_unprepared(settings.name, settings.size, settings.hidden)?;
    let loading_started = Instant::now();
    let (mut loading, game_gpu, parallel) =
        device::Loading::for_game(&extensions, settings.cache, settings.options.ray_tracing)?;
    phases.insert("loading_device_ms", loading_started.elapsed().as_millis());
    phases.insert("device_ready_ms", began.elapsed().as_millis());
    let mut capture = std::env::var_os("ARA_LOADING_CAPTURE").map(PathBuf::from);
    let mut graphics = None;
    let mut graphics_inputs = Some(game_gpu);
    let mut ready_presenter = None;
    let mut prepared = None;
    let mut prepared_world = None;
    let mut startup_frame = None;
    let mut scene_job: Option<graphics::Graphics> = None;
    let mut scene_presenter = None;
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
        if prepared.is_some() && ready_presenter.is_some() {
            let content::Prepared {
                world,
                build_ms,
                frame,
                content,
            } = prepared.take().context("prepared content")?;
            prepared_world = Some((world, build_ms));
            let frame = Arc::new(frame);
            startup_frame = Some(Arc::clone(&frame));
            scene_job = Some(content::warm(
                ready_presenter.take().context("warmed device")?,
                frame,
                content,
            )?);
        }
        if scene_job
            .as_ref()
            .is_some_and(graphics::Graphics::is_finished)
        {
            let (presenter, elapsed) = scene_job.take().context("content worker")?.finish()?;
            *phases.entry("content_warmup_ms").or_default() += elapsed.as_millis();
            phases.insert("content_ready_ms", began.elapsed().as_millis());
            scene_presenter = Some(presenter);
        }
        let drawable = window.drawable_size()?;
        if !drawable.contains(&0)
            && scene_presenter
                .as_ref()
                .is_some_and(|p| p.size() != drawable)
        {
            scene_job = Some(content::resize(
                scene_presenter.take().context("warmed presenter")?,
                Arc::clone(startup_frame.as_ref().context("startup frame")?),
                drawable,
            )?);
        }
        let phase = if prepared.is_some() || prepared_world.is_some() {
            "Preparing renderer"
        } else {
            "Loading scene"
        };
        if phase != shown_phase {
            window.title(&format!("{} — {phase}", settings.name))?;
            shown_phase = phase;
        }
        let completed = u8::from(prepared.is_some() || prepared_world.is_some())
            + u8::from(scene_presenter.is_some());
        // One-queue adapters retain the completed loading image while a worker
        // owns that queue. SDL events still pump; queue access never overlaps.
        let worker_active = graphics.is_some() || scene_job.is_some();
        let may_present = parallel || !worker_active;
        let shown = may_present
            && window.present_loading(&mut loading, 0.1 + f32::from(completed) * 0.4)?;
        if shown {
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
            if let Some(gpu) = graphics_inputs.take() {
                graphics = Some(graphics::Graphics::start(
                    size,
                    gpu,
                    settings.cache.to_path_buf(),
                    settings.quality,
                    settings.options,
                )?);
            }
        }
        // An OUT_OF_DATE/minimized window must recreate successfully behind
        // loading before handoff, never on the first measured game frame.
        if shown && prepared_world.is_some() && scene_presenter.is_some() {
            let (prepared, build_ms) = prepared_world.take().context("prepared runtime")?;
            let world = prepared.map_runtime(|runtime| Ok(crate::net::Play::Local(runtime)))?;
            let mut presenter = scene_presenter.take().context("warmed scene presenter")?;
            presenter.set_probe_gi(true);
            presenter.set_quality(settings.quality);
            window.finish_loading(loading);
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
