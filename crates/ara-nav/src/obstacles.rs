//! Dynamic obstacles: background tile rebakes published after a fixed number of
//! ticks (deterministic), and avoidance discs for avoid-only or not-yet-baked boxes.
use crate::{
    BakeArtifact, BakeInput, Baker, Bounds, NavError, NavMesh, NavObstacle, NavPosition, NavWorld,
    Obstacle, ObstacleMode,
};
use bevy_ecs::prelude::World;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, thread::JoinHandle};

/// Movement below this (metres) does not trigger a rebake.
const TOLERANCE: f32 = 0.05;

type JobResult = Result<(BakeArtifact, NavMesh), NavError>;

struct Source {
    input: Arc<BakeInput>,
    artifact: Arc<BakeArtifact>,
    cache: Option<PathBuf>,
}

struct Job {
    target: Vec<Obstacle>,
    countdown: u32,
    handle: Option<JoinHandle<JobResult>>,
}

/// Per-profile bake sources and in-flight obstacle rebakes (infrastructure owned
/// by `NavWorld`; capture it with [`NavWorld::rebake_checkpoint`]).
#[derive(Default)]
pub(crate) struct Rebakes {
    sources: BTreeMap<u32, Source>,
    jobs: BTreeMap<u32, Job>,
}

/// Serializable obstacle-rebake state for checkpoints: the obstacles baked into
/// each live mesh and any pending rebake with its remaining ticks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RebakeCheckpoint {
    /// Profile -> obstacles baked into the published mesh.
    pub live: BTreeMap<u32, Vec<Obstacle>>,
    /// Profile -> (target obstacles, ticks until publication).
    pub pending: BTreeMap<u32, (Vec<Obstacle>, u32)>,
}

impl NavWorld {
    /// Registers the complete bake input and current artifact of one profile so
    /// carve/area obstacles can rebake it in the background. `input.obstacles` must
    /// be the obstacles baked into `artifact`. Publishes the matching mesh now.
    pub fn set_bake_source(
        &mut self,
        input: BakeInput,
        artifact: BakeArtifact,
        cache: Option<PathBuf>,
    ) -> Result<(), NavError> {
        let mesh = NavMesh::from_artifact(&artifact)?;
        let profile = artifact.settings.agent.id;
        self.meshes.insert(profile, mesh);
        self.rebakes.jobs.remove(&profile);
        self.rebakes.sources.insert(
            profile,
            Source {
                input: Arc::new(input),
                artifact: Arc::new(artifact),
                cache,
            },
        );
        Ok(())
    }

    /// Obstacle-rebake state for a checkpoint provider.
    pub fn rebake_checkpoint(&self) -> RebakeCheckpoint {
        RebakeCheckpoint {
            live: self
                .rebakes
                .sources
                .iter()
                .map(|(p, s)| (*p, s.input.obstacles.clone()))
                .collect(),
            pending: self
                .rebakes
                .jobs
                .iter()
                .map(|(p, j)| (*p, (j.target.clone(), j.countdown)))
                .collect(),
        }
    }

    /// Restores a checkpoint: rebakes live meshes synchronously (blocking; call at
    /// restore, never on a live frame) and restarts pending jobs with their ticks.
    pub fn restore_rebakes(&mut self, checkpoint: &RebakeCheckpoint) -> Result<(), NavError> {
        self.rebakes.jobs.clear();
        for (profile, obstacles) in &checkpoint.live {
            let Some(source) = self.rebakes.sources.get(profile) else {
                continue;
            };
            if &source.input.obstacles != obstacles {
                let (artifact, mesh) =
                    bake(&source.input, &source.artifact, obstacles, &source.cache)?;
                let mut input = (*source.input).clone();
                input.obstacles = obstacles.clone();
                self.meshes.insert(*profile, mesh);
                let source = self
                    .rebakes
                    .sources
                    .get_mut(profile)
                    .expect("source exists");
                source.input = Arc::new(input);
                source.artifact = Arc::new(artifact);
            }
        }
        for (profile, (target, countdown)) in &checkpoint.pending {
            if let Some(source) = self.rebakes.sources.get(profile) {
                let job = spawn(source, target.clone(), *countdown);
                self.rebakes.jobs.insert(*profile, job);
            }
        }
        Ok(())
    }
}

fn bake(
    base: &BakeInput,
    previous: &BakeArtifact,
    obstacles: &[Obstacle],
    cache: &Option<PathBuf>,
) -> JobResult {
    let mut input = base.clone();
    input.obstacles = obstacles.to_vec();
    let artifact = Baker::new(cache.clone()).bake(&input, Some(previous))?;
    let mesh = NavMesh::from_artifact(&artifact)?;
    Ok((artifact, mesh))
}

fn spawn(source: &Source, target: Vec<Obstacle>, countdown: u32) -> Job {
    let (input, artifact, cache) = (
        source.input.clone(),
        source.artifact.clone(),
        source.cache.clone(),
    );
    let obstacles = target.clone();
    let handle = std::thread::Builder::new()
        .name("ara-nav-rebake".into())
        .spawn(move || bake(&input, &artifact, &obstacles, &cache))
        .ok();
    Job {
        target,
        countdown,
        handle,
    }
}

fn bounds(center: [f32; 3], half: [f32; 3]) -> Bounds {
    Bounds {
        min: std::array::from_fn(|i| center[i] - half[i].abs()),
        max: std::array::from_fn(|i| center[i] + half[i].abs()),
    }
}

fn same(a: &[Obstacle], b: &[Obstacle]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.id == b.id
                && a.area == b.area
                && a.bounds
                    .min
                    .iter()
                    .chain(&a.bounds.max)
                    .zip(b.bounds.min.iter().chain(&b.bounds.max))
                    .all(|(x, y)| (x - y).abs() <= TOLERANCE)
        })
}

/// Detects obstacle changes, advances/publishes rebakes and rebuilds the
/// avoidance discs. Runs before agents step, so publication ticks are fixed.
pub(crate) fn update(world: &mut World, nav: &mut NavWorld) {
    let mut views: Vec<(NavObstacle, [f32; 3])> = world
        .query::<(&NavObstacle, &NavPosition)>()
        .iter(world)
        .map(|(o, p)| (*o, p.0))
        .collect();
    views.sort_by_key(|(o, _)| o.id);
    let desired: Vec<Obstacle> = views
        .iter()
        .filter(|(o, p)| o.mode != ObstacleMode::Avoid && crate::finite(*p))
        .map(|(o, p)| Obstacle {
            id: u64::from(o.id),
            bounds: bounds(*p, o.half_extents),
            area: if o.mode == ObstacleMode::Carve {
                Obstacle::CARVE
            } else {
                o.area.min(15)
            },
        })
        .collect();
    let latency = nav.config.rebake_latency_ticks;
    let profiles: Vec<u32> = nav.rebakes.sources.keys().copied().collect();
    for profile in profiles {
        let Some(source) = nav.rebakes.sources.get(&profile) else {
            continue;
        };
        let reference = nav
            .rebakes
            .jobs
            .get(&profile)
            .map_or(&source.input.obstacles, |j| &j.target);
        if !same(reference, &desired) {
            let job = spawn(source, desired.clone(), latency);
            nav.rebakes.jobs.insert(profile, job);
        }
        let Some(job) = nav.rebakes.jobs.get_mut(&profile) else {
            continue;
        };
        if job.countdown > 0 {
            job.countdown -= 1;
            continue;
        }
        let Some(mut job) = nav.rebakes.jobs.remove(&profile) else {
            continue;
        };
        // Due: join the worker (blocks only if it is late), then publish.
        let result = match job.handle.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            _ => Err(NavError::new("rebake", "obstacle rebake worker failed")),
        };
        match result {
            Ok((artifact, mesh)) => {
                if let Some(source) = nav.rebakes.sources.get_mut(&profile) {
                    let mut input = (*source.input).clone();
                    input.obstacles = job.target;
                    source.input = Arc::new(input);
                    source.artifact = Arc::new(artifact);
                }
                nav.meshes.insert(profile, mesh);
            }
            Err(e) => nav.errors.push((0, e)),
        }
    }

    // Avoidance discs: avoid-only boxes, and carve/area boxes not baked yet.
    nav.obstacle_discs.clear();
    for (o, p) in &views {
        if !crate::finite(*p) {
            continue;
        }
        let want = desired.iter().find(|d| d.id == u64::from(o.id));
        let baked = want.is_some_and(|want| {
            !nav.rebakes.sources.is_empty()
                && nav.rebakes.sources.values().all(|s| {
                    s.input
                        .obstacles
                        .iter()
                        .any(|b| same(std::slice::from_ref(b), std::slice::from_ref(want)))
                })
        });
        if baked {
            continue;
        }
        discs(o, *p, &mut nav.obstacle_discs);
    }
}

/// Covers a box footprint with fixed discs along its long axis.
fn discs(o: &NavObstacle, center: [f32; 3], out: &mut Vec<ara_crowd::Agent>) {
    let [hx, hy, hz] = o.half_extents.map(f32::abs);
    let (long, short, along_x) = if hx >= hz {
        (hx, hz, true)
    } else {
        (hz, hx, false)
    };
    let short = short.max(0.05);
    let count = ((long / short).ceil() as usize).clamp(1, 16);
    let step = 2.0 * long / count as f32;
    let radius = (short * short + (step * 0.5) * (step * 0.5)).sqrt();
    for k in 0..count {
        let offset = -long + step * (k as f32 + 0.5);
        let mut position = [center[0], center[1] - hy, center[2]];
        if along_x {
            position[0] += offset;
        } else {
            position[2] += offset;
        }
        out.push(ara_crowd::Agent {
            id: (1 << 63) | (u64::from(o.id) << 8) | k as u64,
            position,
            velocity: [0.0; 3],
            preferred: [0.0; 3],
            radius,
            height: (2.0 * hy).max(0.1),
            max_speed: 0.0,
            max_acceleration: 0.0,
            layers: u32::MAX,
            walls: ara_crowd::NO_WALLS,
            obstacle: true,
        });
    }
}
