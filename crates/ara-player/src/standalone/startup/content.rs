//! The real scene, including its PSOs and initial uploads, stays behind loading.
use super::*;
use ara_runtime::Runtime;
use ara_runtime_render::fidelity::native::Content;

pub(super) struct Prepared {
    pub world: World<Runtime>,
    pub build_ms: f64,
    pub frame: RenderFrame,
    pub content: Content,
}

/// Resolve the first immutable extraction on the runtime worker. No SDL or
/// main-thread input/network owner is attached until this preparation completes.
pub(super) fn prepare(
    scene: &ara_play::PlayScene,
    source: Arc<dyn ara_runtime::assets::AssetSource>,
) -> Result<Prepared> {
    let (mut world, build_ms) = World::prepare_packed(scene, Arc::clone(&source))?;
    let mut frame = RenderFrame::default();
    world.runtime.extract(&mut frame);
    let library = world
        .runtime
        .world()
        .get_resource::<ara_runtime::animation::Library>()
        .context("shipping material library")?;
    let content = Content::resolve(
        &frame,
        world.runtime.documents(),
        world.runtime.schema(),
        library,
        source.as_ref(),
    )?;
    Ok(Prepared {
        world,
        build_ms,
        frame,
        content,
    })
}

/// Move the sole presenter owner to a worker until real content is uploaded and
/// its lazy effect PSOs/systems are ready. Loading keeps presenting on main.
pub(super) fn warm(
    mut presenter: Presenter,
    frame: Arc<RenderFrame>,
    content: Content,
) -> Result<graphics::Graphics> {
    graphics::Graphics::spawn(move || {
        ara_gpu::cpu_scope!("Player.content_warmup_worker");
        presenter.set_probe_gi(true);
        presenter.present_with(&frame, || Ok(content))?;
        loop {
            let resources_ready = presenter.preallocate_startup_resources()?;
            if !presenter.readiness().pipelines_pending && resources_ready {
                break;
            }
            presenter.present_with(&frame, || {
                anyhow::bail!("unchanged startup content unexpectedly rebuilt")
            })?;
            if let Some(error) = presenter.presentation_error() {
                anyhow::bail!("startup presentation: {error}");
            }
        }
        if let Some(error) = presenter.presentation_error() {
            anyhow::bail!("startup presentation: {error}");
        }
        Ok(presenter)
    })
}

/// A resize during loading must warm the replacement targets before handoff.
/// The same content and PSOs stay resident; no source resolution runs again.
pub(super) fn resize(
    mut presenter: Presenter,
    frame: Arc<RenderFrame>,
    size: [u32; 2],
) -> Result<graphics::Graphics> {
    graphics::Graphics::spawn(move || {
        presenter.resize(size)?;
        presenter.present_with(&frame, || {
            anyhow::bail!("resized startup content unexpectedly rebuilt")
        })?;
        if let Some(error) = presenter.presentation_error() {
            anyhow::bail!("startup resize: {error}");
        }
        Ok(presenter)
    })
}
