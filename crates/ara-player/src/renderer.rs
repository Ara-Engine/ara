//! Renderlab behind every presenter (`--renderer`, `ARA_RENDERER`). The choice is
//! fixed for the process at launch; every presenter the player creates passes through
//! [`install`], so windows, the editor's embedded Play (its Game view), captures and
//! perf runs all follow it. A Renderlab that cannot start is `ARA_RENDERLAB_UNAVAILABLE`.
//! The editor's Scene view does not go through the player.
use anyhow::Result;
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime_render::Presenter;
use std::{path::Path, sync::OnceLock};

/// Which Renderlab path draws the 3D scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub(crate) enum RendererChoice {
    /// Hardware ray tracing where the GPU has it, else the non-RT path. On Apple,
    /// Renderlab's ara-gpu backend on the presenter's Metal device.
    #[default]
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

/// Puts Renderlab behind `presenter`; `cache` is the player's GPU cache directory.
/// A Renderlab that cannot start is an error (`ARA_RENDERLAB_UNAVAILABLE`).
pub(crate) fn install(presenter: &mut Presenter, cache: &Path) -> Result<()> {
    let path = match CHOICE.get().copied().unwrap_or_default() {
        RendererChoice::Renderlab => PathChoice::Auto,
        RendererChoice::RenderlabRt => PathChoice::HardwareRt,
        RendererChoice::RenderlabNonrt => PathChoice::NonRt,
    };
    if presenter.external_renderer().is_some() {
        return Ok(());
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
        // The player draws no Scene overlays: no id pass.
        scene_ids: false,
    };
    Renderlab::new(&options)
        .and_then(|renderlab| presenter.set_external_renderer(Some(Box::new(renderlab))))
        .map_err(|error| {
            anyhow::anyhow!("ARA_RENDERLAB_UNAVAILABLE: Renderlab could not start: {error:#}")
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    /// Renderlab is the default. `ara` is not a choice.
    #[test]
    fn renderlab_is_the_default_and_ara_is_rejected() {
        assert_eq!(RendererChoice::default(), RendererChoice::Renderlab);
        for (name, choice) in [
            ("renderlab", RendererChoice::Renderlab),
            ("renderlab-rt", RendererChoice::RenderlabRt),
            ("renderlab-nonrt", RendererChoice::RenderlabNonrt),
        ] {
            assert_eq!(
                RendererChoice::from_str(name, false).expect("a documented name"),
                choice,
                "{name}"
            );
        }
        assert!(RendererChoice::from_str("ara", false).is_err());
    }
}
