//! The frame's sky on Renderlab's engine, on either target: the atmosphere, or an
//! authored sky image (an HDRI or the gradient sky) as `Sky::Map`. An image is prepared
//! once per content (`environment::prepare`) on a worker thread, a few hundred
//! milliseconds of filtering; the frame loop keeps the sky it has (a constant of the
//! image's mean before the first one) until the new one is ready. A capture or a cut
//! waits for it, so stills never show a sky in transition.
use crate::{
    environment,
    gaps::Gaps,
    lighting::{self, SkySource},
    rl::Engine,
};
use anyhow::{Context, Result, anyhow};
use ara_runtime_render::ExternalView;
use renderlab::engine::{Background, EnvironmentId, PreparedEnvironment, Sky};

/// One sky image being prepared for `Sky::Map` on a worker thread.
struct Job {
    /// Content key of the image (`environment::key`).
    key: u64,
    worker: std::thread::JoinHandle<Result<PreparedEnvironment>>,
}

/// The sky last set on the engine, and the maps it prepares and retires.
#[derive(Default)]
pub(crate) struct Skies {
    /// A sky image lights the scene as a constant of its mean radiance (`Options`).
    constant: bool,
    applied: Option<Sky>,
    /// The prepared map in use and the content key it was prepared from.
    map: Option<(u64, EnvironmentId)>,
    job: Option<Job>,
    /// Replaced maps waiting for the next sky change to be removed.
    stale: Vec<EnvironmentId>,
}

impl Skies {
    pub fn new(constant: bool) -> Self {
        Self {
            constant,
            ..Self::default()
        }
    }

    /// Sets the frame's sky on `engine` when it changed, and says whether it did. `wait` (a
    /// capture or a cut) waits for a sky image being prepared.
    pub fn apply(
        &mut self,
        engine: &mut Engine,
        view: &ExternalView<'_>,
        gaps: &mut Gaps,
        wait: bool,
    ) -> Result<bool> {
        let mut replaced = None;
        let sky = match lighting::sky(view, gaps) {
            SkySource::Atmosphere(medium) => Sky::Atmosphere(medium),
            SkySource::Map { size, pixels } if self.constant => {
                Sky::Uniform(environment::mean_radiance(size, &pixels))
            }
            SkySource::Map { size, pixels } => {
                let key = environment::key(size, &pixels);
                loop {
                    // Take over a finished preparation (any, when waiting).
                    if let Some(job) = self.job.take_if(|job| wait || job.worker.is_finished()) {
                        let prepared = job
                            .worker
                            .join()
                            .map_err(|_| anyhow!("the sky preparation thread panicked"))??;
                        let map = engine.create_environment(&prepared)?;
                        let previous = self.map.replace((job.key, map));
                        // Two maps can finish between sky changes: all but the last go.
                        if let Some(stale) = replaced.replace(previous.map(|(_, map)| map)) {
                            self.stale.extend(stale);
                        }
                    }
                    let current = self.map.map(|(applied, _)| applied);
                    if current == Some(key) || self.job.is_some() {
                        break;
                    }
                    let owned = pixels.to_vec();
                    let worker = std::thread::Builder::new()
                        .name("renderlab-sky".into())
                        .spawn(move || environment::prepare(size, &owned))
                        .context("sky preparation thread")?;
                    self.job = Some(Job { key, worker });
                    if !wait {
                        break;
                    }
                }
                let light = lighting::sky_light(view);
                match self.map {
                    Some((_, map)) => Sky::Map {
                        map,
                        rotation_y: 0.,
                        intensity: light,
                        background: if light == 1. {
                            Background::Sky
                        } else {
                            Background::ScaledSky(1.)
                        },
                    },
                    None => Sky::Uniform(environment::mean_radiance(size, &pixels)),
                }
            }
        };
        let changed = self.applied != Some(sky);
        if changed {
            engine.set_sky(sky)?;
            self.applied = Some(sky);
        }
        // Renderlab keeps the selected map alive until the sky changes: old ones go
        // only after the new sky is set.
        for map in replaced.flatten().into_iter().chain(self.stale.drain(..)) {
            engine.remove_environment(map)?;
        }
        Ok(changed)
    }
}
