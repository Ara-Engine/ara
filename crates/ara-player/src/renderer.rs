//! The renderer switch (`--renderer`, `ARA_RENDERER`): Ara's own renderer by default,
//! or Renderlab behind the same presenter (`ara-renderlab`). The choice is fixed for
//! the process at launch; every presenter the player creates passes through
//! [`install`], so windows, the editor's embedded Play, captures and perf runs all
//! follow it.
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime_render::Presenter;
use std::{path::Path, sync::OnceLock};

/// Which renderer draws the 3D scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub(crate) enum RendererChoice {
    /// Ara's renderer (`ara-render`).
    #[default]
    Ara,
    /// Renderlab: hardware ray tracing where the GPU has it, else its non-RT path.
    Renderlab,
    /// Renderlab's hardware ray-traced path.
    RenderlabRt,
    /// Renderlab's non-RT path (`ARA_RENDERLAB_NONRT_PATENT`: its sun shadows await a patent review).
    RenderlabNonrt,
}

static CHOICE: OnceLock<RendererChoice> = OnceLock::new();

/// Records the launch's choice. Call once, before any presenter exists.
pub(crate) fn configure(choice: RendererChoice) {
    let _ = CHOICE.set(choice);
}

/// Puts the chosen renderer behind `presenter`; `cache` is the player's GPU cache
/// directory. A Renderlab that cannot start reports why (`ARA_RENDERLAB_UNAVAILABLE`) and leaves Ara's
/// renderer drawing.
pub(crate) fn install(presenter: &mut Presenter, cache: &Path) {
    let path = match CHOICE.get().copied().unwrap_or_default() {
        RendererChoice::Ara => return,
        RendererChoice::Renderlab => PathChoice::Auto,
        RendererChoice::RenderlabRt => PathChoice::HardwareRt,
        RendererChoice::RenderlabNonrt => PathChoice::NonRt,
    };
    if presenter.external_renderer().is_some() {
        return;
    }
    let options = Options {
        path,
        pipeline_cache: Some(cache.join("renderlab")),
        // An A/B switch for Renderlab's traced-reflection band (`Options`).
        max_reflection_roughness: std::env::var("ARA_RENDERLAB_REFLECTION_ROUGHNESS")
            .ok()
            .and_then(|value| value.parse().ok()),
        // `ARA_RENDERLAB_HANDOFF=copy` forces the CPU copy, to measure it against sharing.
        cpu_copy_handoff: std::env::var("ARA_RENDERLAB_HANDOFF").is_ok_and(|value| value == "copy"),
    };
    let installed = Renderlab::new(&options)
        .and_then(|renderlab| presenter.set_external_renderer(Some(Box::new(renderlab))));
    if let Err(error) = installed {
        eprintln!("{error:#}; this view renders with Ara's renderer");
    }
}
