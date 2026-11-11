//! The renderer switch (`--renderer`, `ARA_RENDERER`): Renderlab behind the presenter
//! (`ara-renderlab`) by default, with Ara's own renderer (`ara-render`, frozen) where Renderlab
//! cannot run or when asked for. The choice is fixed for the process at launch; every
//! presenter the player creates passes through [`install`], so windows, the editor's embedded
//! Play (its Game view), captures and perf runs all follow it. The editor's Scene view does
//! not go through the player and still draws with `ara-render`.
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime_render::Presenter;
use std::{
    path::Path,
    sync::{Once, OnceLock},
};

/// Which renderer draws the 3D scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub(crate) enum RendererChoice {
    /// Renderlab: hardware ray tracing where the GPU has it, else its non-RT path; on
    /// Apple, its ara-gpu backend on the presenter's Metal device (hardware RT only). Ara's
    /// renderer draws where Renderlab cannot start.
    #[default]
    Renderlab,
    /// Ara's renderer (`ara-render`), frozen: kept for where Renderlab cannot run and for
    /// comparison.
    Ara,
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

/// Says once per process which renderer draws and why, when it is Ara's: Renderlab's own
/// start-up line (`Ara presentation: Renderlab (...) on <adapter>`) covers the other case.
fn ara_draws(reason: &str) {
    static SAID: Once = Once::new();
    SAID.call_once(|| eprintln!("Ara presentation: ara-render draws ({reason})"));
}

/// Puts the chosen renderer behind `presenter`; `cache` is the player's GPU cache
/// directory. A Renderlab that cannot start reports why (`ARA_RENDERLAB_UNAVAILABLE`) and
/// leaves Ara's renderer drawing.
pub(crate) fn install(presenter: &mut Presenter, cache: &Path) {
    let path = match CHOICE.get().copied().unwrap_or_default() {
        RendererChoice::Ara => {
            return ara_draws("selected with --renderer ara or ARA_RENDERER=ara");
        }
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
        // `ARA_RENDERLAB_SKY=constant` measures what a sky map costs in Renderlab.
        constant_sky: std::env::var("ARA_RENDERLAB_SKY").is_ok_and(|value| value == "constant"),
    };
    let installed = Renderlab::new(&options)
        .and_then(|renderlab| presenter.set_external_renderer(Some(Box::new(renderlab))));
    if let Err(error) = installed {
        ara_draws(&format!("Renderlab could not start: {error:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    /// Renderlab is the default and every documented name selects what it says.
    #[test]
    fn renderlab_is_the_default_and_every_name_parses() {
        assert_eq!(RendererChoice::default(), RendererChoice::Renderlab);
        for (name, choice) in [
            ("renderlab", RendererChoice::Renderlab),
            ("ara", RendererChoice::Ara),
            ("renderlab-rt", RendererChoice::RenderlabRt),
            ("renderlab-nonrt", RendererChoice::RenderlabNonrt),
        ] {
            assert_eq!(
                RendererChoice::from_str(name, false).expect("a documented name"),
                choice,
                "{name}"
            );
        }
    }
}
