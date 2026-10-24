//! Probe GI acquisition for the raster path. A content generation's static
//! instances are traced once into probe surfaces, on a ray-tracing device where
//! available (`bake_probe_surfaces` on a temporary fidelity view) or on the CPU
//! (`gi_cpu`); the raster renderer relights them every frame without rays.
//!
//! [`Acquisition`] runs one job at a time on a worker thread, caches full bakes
//! on disk by content key, and keeps the worker's CPU tracer between jobs, so
//! edits never rebake everything: moving, adding or removing instances re-traces
//! only the probes they can change, and the renderer blends those in while the
//! stale surfaces stay in use. Dynamic entities ([`Mobility`]: recently moved)
//! and deforming geometry stay out of the surfaces; the relit volume lights them.
use crate::fidelity::native::{self, Content, NativeConfig};
use crate::gi_cpu::{self, Tracer};
use anyhow::{Context, Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::instanced::Scene;
use ara_render::look::gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS};
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler,
    environment::FrameEnvironment,
};
use ara_runtime::extract::RenderFrame;
use glam30::Mat4;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
};

/// Bumped whenever the bake or the cache layout changes.
const VERSION: u32 = 2;
const MAGIC: &[u8; 8] = b"ARAGI\0\0\x01";

/// Residency used only while baking (full-resolution albedo for the caches).
fn config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1024 * 1024 * 1024,
        upload_bytes: 64 * 1024 * 1024,
    }
}

/// Presented frames an instance must stay still before it is traced into the
/// probe surfaces again (half a second at 60 Hz).
const REST_FRAMES: u32 = 30;

/// Splits a view's instances for probe GI: an instance that moved within the
/// last [`REST_FRAMES`] frames is dynamic, kept out of the surfaces and lit by
/// them instead. Counting frames (not time) keeps captures deterministic.
#[derive(Default)]
pub(crate) struct Mobility {
    /// Per scene instance: entity, last world and frames since it moved.
    seen: Vec<(u64, Mat4, u32)>,
    dynamic: BTreeSet<u64>,
}

impl Mobility {
    /// Observes this frame's instances (`moved`: any pose changed or a new
    /// generation); returns whether the dynamic set changed. New instances start
    /// at rest; a new generation keeps the counts of the entities it retains.
    pub(crate) fn observe(&mut self, scene: &Scene, moved: bool) -> bool {
        if !moved && self.dynamic.is_empty() {
            return false;
        }
        if self.seen.len() != scene.instances.len()
            || self
                .seen
                .iter()
                .zip(&scene.instances)
                .any(|(s, i)| s.0 != i.entity)
        {
            let old: HashMap<u64, (Mat4, u32)> =
                self.seen.drain(..).map(|(e, w, r)| (e, (w, r))).collect();
            self.seen = scene
                .instances
                .iter()
                .map(|i| {
                    let rest = old.get(&i.entity).map_or(REST_FRAMES, |&(world, rest)| {
                        if world == i.world { rest } else { 0 }
                    });
                    (i.entity, i.world, rest)
                })
                .collect();
        } else {
            for (seen, instance) in self.seen.iter_mut().zip(&scene.instances) {
                if seen.1 != instance.world {
                    seen.1 = instance.world;
                    seen.2 = 0;
                } else if seen.2 < REST_FRAMES {
                    seen.2 += 1;
                }
            }
        }
        let dynamic: BTreeSet<u64> = self
            .seen
            .iter()
            .filter(|s| s.2 < REST_FRAMES)
            .map(|s| s.0)
            .collect();
        let changed = dynamic != self.dynamic;
        self.dynamic = dynamic;
        changed
    }

    /// Entities kept out of the probe surfaces.
    pub(crate) fn dynamic(&self) -> &BTreeSet<u64> {
        &self.dynamic
    }

    /// Whether an instance is still counting down to rest: a view that otherwise
    /// stops redrawing must keep presenting until it settles.
    pub(crate) fn settling(&self) -> bool {
        !self.dynamic.is_empty()
    }

    /// Current instance transforms by entity.
    pub(crate) fn worlds(&self) -> Vec<(u64, Mat4)> {
        self.seen.iter().map(|s| (s.0, s.1)).collect()
    }
}

/// One acquisition: a content generation (`key`, its frame and assets) and the
/// entities kept out; an edit also carries the current transforms.
struct Job {
    serial: u64,
    key: u64,
    frame: Arc<RenderFrame>,
    content: Content,
    dynamic: BTreeSet<u64>,
    edit: Option<Vec<(u64, Mat4)>>,
}

/// Worker state between jobs: the surfaces last delivered, their generation and
/// the CPU tracer of their scene once built.
struct Worker {
    key: u64,
    surfaces: ProbeSurfaces,
    tracer: Option<Tracer>,
}

enum Message {
    /// The job's surfaces (delivered as soon as they exist).
    Surfaces(Result<ProbeSurfaces>),
    /// The job finished, returning the worker state.
    Done(Option<Worker>),
}

struct Active {
    serial: u64,
    delivered: bool,
    receive: Receiver<Message>,
}

/// Background probe surface acquisition for one view. At most one job runs at a
/// time; while it runs, only the newest request waits (bursts of publications or
/// edits never stack bakes), and a superseded result is discarded.
pub(crate) struct Acquisition {
    cache: PathBuf,
    /// Trace on the CPU even where ray queries exist (`ARA_PROBE_GI_CPU=1`).
    cpu: bool,
    /// Newest generation key (content key and bake version).
    key: Option<u64>,
    /// The newest generation's frame and assets, for edits within it.
    source: Option<(Arc<RenderFrame>, Content)>,
    /// Serial of the newest request.
    newest: u64,
    active: Option<Active>,
    queued: Option<Job>,
    /// Worker state while no job runs.
    worker: Option<Worker>,
}

impl Acquisition {
    pub(crate) fn new(cache: PathBuf) -> Self {
        Self::with_method(cache, std::env::var_os("ARA_PROBE_GI_CPU").is_some())
    }

    /// `cpu` forces the CPU bake on ray-tracing adapters too; each method caches
    /// separately.
    pub(crate) fn with_method(cache: PathBuf, cpu: bool) -> Self {
        Self {
            cache: cache.join("probe-gi"),
            cpu,
            key: None,
            source: None,
            newest: 0,
            active: None,
            queued: None,
            worker: None,
        }
    }

    /// Requests `frame`'s content generation `key` with `dynamic` entities kept
    /// out, unless it is already the newest (edits cover changes within one).
    /// `content` is only resolved for a new key.
    pub(crate) fn request(
        &mut self,
        key: u64,
        frame: &RenderFrame,
        content: impl FnOnce() -> Result<Content>,
        dynamic: &BTreeSet<u64>,
    ) -> Result<()> {
        let key = key ^ u64::from(VERSION);
        if self.key == Some(key) {
            return Ok(());
        }
        self.key = Some(key);
        let (frame, content) = (Arc::new(snapshot(frame)), content()?);
        self.source = Some((frame.clone(), content.clone()));
        self.submit(key, frame, content, dynamic, None)
    }

    /// Requests an edit within the newest generation: current instance `worlds`
    /// (by entity) and the new `dynamic` set. Ignored before any generation.
    pub(crate) fn edit(&mut self, worlds: Vec<(u64, Mat4)>, dynamic: &BTreeSet<u64>) -> Result<()> {
        let (Some(key), Some((frame, content))) = (self.key, &self.source) else {
            return Ok(());
        };
        let (frame, content) = (frame.clone(), content.clone());
        self.submit(key, frame, content, dynamic, Some(worlds))
    }

    fn submit(
        &mut self,
        key: u64,
        frame: Arc<RenderFrame>,
        content: Content,
        dynamic: &BTreeSet<u64>,
        edit: Option<Vec<(u64, Mat4)>>,
    ) -> Result<()> {
        self.newest += 1;
        let job = Job {
            serial: self.newest,
            key,
            frame,
            content,
            dynamic: dynamic.clone(),
            edit,
        };
        if self.active.is_some() {
            self.queued = Some(job);
            return Ok(());
        }
        self.start(job)
    }

    /// Whether newer surfaces are still to come: a view that otherwise stops
    /// redrawing must keep polling until they arrive.
    pub(crate) fn is_pending(&self) -> bool {
        self.queued.is_some() || self.active.as_ref().is_some_and(|a| !a.delivered)
    }

    fn start(&mut self, job: Job) -> Result<()> {
        let (cache, cpu, worker) = (self.cache.clone(), self.cpu, self.worker.take());
        let (send, receive) = channel();
        let serial = job.serial;
        std::thread::Builder::new()
            .name("probe-gi".into())
            .spawn(move || run(&cache, cpu, job, worker, &send))
            .context("starting the probe GI worker")?;
        self.active = Some(Active {
            serial,
            delivered: false,
            receive,
        });
        Ok(())
    }

    /// The newest request's surfaces, once, when they arrive; `wait` blocks until
    /// they do. Failures are reported and leave the last surfaces in use.
    pub(crate) fn poll(&mut self, wait: bool) -> Option<ProbeSurfaces> {
        loop {
            let active = self.active.as_mut()?;
            let block = wait && (!active.delivered || self.queued.is_some());
            let message = if block {
                active.receive.recv().ok()
            } else {
                match active.receive.try_recv() {
                    Ok(message) => Some(message),
                    Err(TryRecvError::Empty) => return None,
                    Err(TryRecvError::Disconnected) => None,
                }
            };
            match message {
                Some(Message::Surfaces(result)) => {
                    active.delivered = true;
                    match result {
                        Ok(surfaces) if active.serial == self.newest => return Some(surfaces),
                        // Superseded by a newer request.
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("ara-runtime-render: probe GI acquisition failed: {error:#}")
                        }
                    }
                }
                done => {
                    match done {
                        Some(Message::Done(worker)) => self.worker = worker,
                        _ => eprintln!("ara-runtime-render: probe GI worker stopped"),
                    }
                    self.active = None;
                    if let Some(next) = self.queued.take()
                        && let Err(error) = self.start(next)
                    {
                        eprintln!("ara-runtime-render: probe GI acquisition failed: {error:#}");
                    }
                }
            }
        }
    }
}

/// The frame fields a bake reads (draws, terrain, lights, look, camera).
fn snapshot(frame: &RenderFrame) -> RenderFrame {
    let mut copy = RenderFrame::default();
    copy.tick = frame.tick;
    copy.camera = frame.camera;
    copy.environment = frame.environment;
    copy.look = frame.look;
    copy.sun = frame.sun;
    copy.lights = frame.lights.clone();
    copy.draws = frame.draws.clone();
    copy.palettes = frame.palettes.clone();
    copy.terrain = frame.terrain.clone();
    copy
}

/// After delivery, the worker still needs a tracer of the delivered scene:
/// `prepared` when the job already has it, adopting an older `tracer` if any.
struct FollowUp {
    prepared: Option<native::Prepared>,
    tracer: Option<Tracer>,
}

/// Runs `job` on the worker thread: delivers its surfaces, then gives the worker
/// a tracer of their scene (for the next edit) before handing the state back.
fn run(cache: &Path, cpu: bool, job: Job, worker: Option<Worker>, send: &Sender<Message>) {
    let mut worker = worker;
    let started = std::time::Instant::now();
    // One line per job (kind, thread, outcome) so hosts can audit where GI work runs.
    let note = |outcome: &str| {
        eprintln!(
            "ara-runtime-render: probe GI job {} ({}) on thread {}: {outcome} in {:.2} s",
            job.serial,
            if job.edit.is_some() {
                "edit"
            } else {
                "generation"
            },
            std::thread::current().name().unwrap_or("unnamed"),
            started.elapsed().as_secs_f64()
        )
    };
    let follow = match acquire(cache, cpu, &job, &mut worker) {
        Ok((surfaces, follow)) => {
            let _ = send.send(Message::Surfaces(Ok(surfaces)));
            note("delivered");
            follow
        }
        Err(error) => {
            let _ = send.send(Message::Surfaces(Err(error)));
            note("failed");
            None
        }
    };
    if let Some(follow) = follow
        && let Some(state) = worker.as_mut()
    {
        let tracer = follow
            .prepared
            .map_or_else(|| prepare(&job), Ok)
            .map(|prepared| {
                let shading = shading(&prepared);
                match follow.tracer {
                    Some(mut tracer) => {
                        tracer.adopt(&prepared.scene, &shading, &job.dynamic);
                        tracer
                    }
                    None => Tracer::new(&prepared.scene, &shading, &job.dynamic),
                }
            });
        match tracer {
            Ok(tracer) => state.tracer = Some(tracer),
            Err(error) => eprintln!("ara-runtime-render: probe GI edits unavailable: {error:#}"),
        }
    }
    let _ = send.send(Message::Done(worker));
}

/// The job's native scene, with an edit's transforms applied.
fn prepare(job: &Job) -> Result<native::Prepared> {
    let mut prepared = native::prepare(&job.frame, job.content.clone(), config())?;
    if let Some(worlds) = &job.edit {
        let worlds: HashMap<u64, Mat4> = worlds.iter().copied().collect();
        for instance in &mut prepared.scene.instances {
            if let Some(&world) = worlds.get(&instance.entity) {
                instance.world = world;
            }
        }
    }
    Ok(prepared)
}

/// Cache-key part of the dynamic set (zero when empty).
fn dynamic_key(dynamic: &BTreeSet<u64>) -> u64 {
    let seed = if dynamic.is_empty() { 0 } else { crate::FNV };
    dynamic
        .iter()
        .fold(seed, |key, e| crate::fnv(key, &e.to_le_bytes()))
}

/// Produces the job's surfaces: an edit re-traces in place; a generation is
/// incremental against the previous tracer where the lattice still fits and
/// under half the probes change, else loaded from the cache or fully baked (on
/// a new ray-tracing device where available). Also returns what the worker
/// still needs for a tracer of the delivered scene.
fn acquire(
    cache: &Path,
    cpu: bool,
    job: &Job,
    worker: &mut Option<Worker>,
) -> Result<(ProbeSurfaces, Option<FollowUp>)> {
    if let Some(worlds) = &job.edit
        && let Some(state) = worker.as_mut().filter(|w| w.key == job.key)
        && let Some(tracer) = state.tracer.as_mut()
    {
        let started = std::time::Instant::now();
        let boxes = tracer.edit(worlds, &job.dynamic);
        let probes = gi_cpu::affected(&state.surfaces, &boxes);
        gi_cpu::rebake(tracer, &mut state.surfaces, &probes)?;
        eprintln!(
            "ara-runtime-render: re-traced {} of {} GI probes after an edit in {:.2} s",
            probes.len(),
            state.surfaces.len(),
            started.elapsed().as_secs_f64()
        );
        return Ok((state.surfaces.clone(), None));
    }
    let method = if cpu { "cpu" } else { "auto" };
    let path = cache.join(format!(
        "{:016x}-{method}.gi",
        job.key ^ dynamic_key(&job.dynamic)
    ));
    let mut tracer = worker.as_mut().and_then(|w| w.tracer.take());
    if let Ok(bytes) = std::fs::read(&path) {
        match decode(&bytes) {
            Ok(surfaces) => {
                *worker = Some(Worker {
                    key: job.key,
                    surfaces: surfaces.clone(),
                    tracer: None,
                });
                return Ok((
                    surfaces,
                    Some(FollowUp {
                        prepared: None,
                        tracer,
                    }),
                ));
            }
            Err(error) => eprintln!(
                "ara-runtime-render: ignoring probe GI cache {}: {error:#}",
                path.display()
            ),
        }
    }
    let mut prepared = prepare(job)?;
    let shading = shading(&prepared);
    if let Some(adopting) = tracer.as_mut()
        && let Some(state) = worker.as_mut()
    {
        let started = std::time::Instant::now();
        let boxes = adopting.adopt(&prepared.scene, &shading, &job.dynamic);
        let fits = adopting
            .bounds()
            .is_some_and(|bounds| gi_cpu::covers(&state.surfaces, bounds));
        let probes = gi_cpu::affected(&state.surfaces, &boxes);
        if fits && probes.len() * 2 <= state.surfaces.len() {
            gi_cpu::rebake(adopting, &mut state.surfaces, &probes)?;
            eprintln!(
                "ara-runtime-render: re-traced {} of {} GI probes for new content in {:.2} s",
                probes.len(),
                state.surfaces.len(),
                started.elapsed().as_secs_f64()
            );
            state.key = job.key;
            state.tracer = tracer.take();
            return Ok((state.surfaces.clone(), None));
        }
    }
    let traced = (!cpu)
        .then(|| VulkanDevice::new(&[], cache.join("device")).ok())
        .flatten()
        .filter(|gpu| gpu.context().info.ray_tracing);
    let surfaces = match traced {
        Some(mut gpu) => {
            // Only static rigid instances enter the ray-traced bake.
            let scene = &mut prepared.scene;
            let all = std::mem::take(&mut scene.instances);
            let (baked, excluded): (Vec<_>, Vec<_>) = all.into_iter().partition(|i| {
                !job.dynamic.contains(&i.entity)
                    && scene.geometry.get(i.geometry).is_some_and(|g| !g.deforming)
            });
            scene.instances = baked;
            let surfaces = bake(&mut gpu, &job.frame, &prepared);
            prepared.scene.instances.extend(excluded);
            surfaces?
        }
        None => {
            let tracer =
                tracer.get_or_insert_with(|| Tracer::new(&prepared.scene, &shading, &job.dynamic));
            gi_cpu::bake(tracer)?
        }
    };
    std::fs::create_dir_all(cache)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, encode(&surfaces))?;
    std::fs::rename(&temporary, &path)?;
    let follow = tracer.is_none().then_some(FollowUp {
        prepared: Some(prepared),
        tracer: None,
    });
    *worker = Some(Worker {
        key: job.key,
        surfaces: surfaces.clone(),
        tracer,
    });
    Ok((surfaces, follow))
}

fn encode(s: &ProbeSurfaces) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for v in s.origin.iter().chain(&s.spacing) {
        bytes.extend(v.to_le_bytes());
    }
    for c in s.counts {
        bytes.extend(c.to_le_bytes());
    }
    bytes.extend_from_slice(bytemuck::cast_slice(&s.texels));
    bytes.extend_from_slice(bytemuck::cast_slice(&s.statics));
    bytes
}

fn decode(bytes: &[u8]) -> Result<ProbeSurfaces> {
    ensure!(
        bytes.len() >= 44 && &bytes[..8] == MAGIC,
        "not a probe GI cache"
    );
    let word = |i: usize| -> [u8; 4] {
        [
            bytes[8 + i * 4],
            bytes[9 + i * 4],
            bytes[10 + i * 4],
            bytes[11 + i * 4],
        ]
    };
    let float = |i| f32::from_le_bytes(word(i));
    let counts = [6, 7, 8].map(|i| u32::from_le_bytes(word(i)));
    let probes = counts.iter().map(|&c| c as usize).product::<usize>();
    let texel_bytes = probes * SURFACE_TEXELS * 16;
    ensure!(
        bytes.len() == 44 + texel_bytes + probes * STATIC_VECTORS * 16,
        "truncated probe GI cache"
    );
    let surfaces = ProbeSurfaces {
        origin: [float(0), float(1), float(2)],
        spacing: [float(3), float(4), float(5)],
        counts,
        texels: bytemuck::pod_collect_to_vec(&bytes[44..44 + texel_bytes]),
        statics: bytemuck::pod_collect_to_vec(&bytes[44 + texel_bytes..]),
    };
    surfaces.validate()?;
    Ok(surfaces)
}

/// Bakes the probe surfaces of `frame`'s content on a ray-tracing device.
/// Each material's diffuse albedo (metal removed) and emissive radiance for the
/// CPU bake: factors times the mean texel of their maps; terrain splats average
/// their layer materials.
fn shading(prepared: &native::Prepared) -> Vec<crate::gi_cpu::Shading> {
    use ara_render::instanced::NO_TEXTURE;
    use glam30::Vec3;
    let missing = std::cell::Cell::new(0usize);
    let mean = |id: u32| {
        if id == NO_TEXTURE {
            return [1.; 4];
        }
        prepared.textures.mean_texel(id).unwrap_or_else(|| {
            missing.set(missing.get() + 1);
            [1.; 4]
        })
    };
    let materials = &prepared.scene.materials;
    let own: Vec<_> = materials
        .iter()
        .map(|m| {
            let (c, e) = (m.pbr.base_color, m.pbr.emissive_roughness);
            let (t, et) = (mean(m.pbr.textures[0]), mean(m.pbr.textures[3]));
            // glTF metalness lives in the blue channel of the metal/roughness map.
            let metal = (m.pbr.surface[0] * mean(m.pbr.textures[2])[2]).clamp(0., 1.);
            crate::gi_cpu::Shading {
                albedo: Vec3::new(c[0] * t[0], c[1] * t[1], c[2] * t[2]) * (1. - metal),
                emissive: Vec3::new(e[0] * et[0], e[1] * et[1], e[2] * et[2]),
            }
        })
        .collect();
    if missing.get() > 0 {
        eprintln!(
            "ara-runtime-render: CPU probe bake: {} maps have no CPU mean (taken as white)",
            missing.get()
        );
    }
    materials
        .iter()
        .zip(&own)
        .map(|(m, &shading)| {
            if m.extras.terrain_layers == 0 {
                return shading;
            }
            let layers: Vec<_> = m
                .extras
                .layer_materials
                .iter()
                .filter_map(|&id| own.get(id as usize))
                .collect();
            let n = layers.len().max(1) as f32;
            crate::gi_cpu::Shading {
                albedo: layers.iter().map(|s| s.albedo).sum::<Vec3>() / n,
                emissive: layers.iter().map(|s| s.emissive).sum::<Vec3>() / n,
            }
        })
        .collect()
}

fn bake(
    gpu: &mut VulkanDevice,
    frame: &RenderFrame,
    prepared: &native::Prepared,
) -> Result<ProbeSurfaces> {
    let started = std::time::Instant::now();
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    let mut view = FidelityView::new_native(
        gpu,
        &prepared.scene,
        prepared.textures.fork_sources(config())?,
        Settings {
            render_size: [64, 36],
            display_size: [64, 36],
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 1.,
            post: look.post(PostSettings::default()),
        },
        None,
    )?;
    let surfaces = (|| {
        view.set_environment(&look)?;
        for _ in 0..256 {
            if view.refine_native_textures(gpu)?.pending_textures == 0 {
                break;
            }
        }
        let camera = crate::camera(frame)?;
        view.render(
            gpu,
            &FrameInput {
                camera: &camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        view.bake_probe_surfaces(gpu)
    })();
    view.destroy(gpu)?;
    let surfaces = surfaces?;
    ensure!(!surfaces.is_empty(), "empty probe bake");
    eprintln!(
        "ara-runtime-render: baked {} GI probes ({:?}) in {:.1} s",
        surfaces.len(),
        surfaces.counts,
        started.elapsed().as_secs_f64()
    );
    Ok(surfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice(origin: f32) -> ProbeSurfaces {
        ProbeSurfaces {
            origin: [origin, 0., 0.],
            spacing: [1.; 3],
            counts: [2, 2, 2],
            texels: vec![[0; 4]; 8 * SURFACE_TEXELS],
            statics: vec![[1., 1., 0., 1.]; 8 * STATIC_VECTORS],
        }
    }

    #[test]
    fn bursts_run_one_bake_and_deliver_only_the_newest_request() -> Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/runtime-render/probe-gi-queue");
        let _ = std::fs::remove_dir_all(&root);
        let mut acquisition = Acquisition::new(root.clone());
        // Cached results make every "bake" a file read, so no GPU is needed.
        std::fs::create_dir_all(root.join("probe-gi"))?;
        for (key, origin) in [(1u64, 1.), (2, 2.), (3, 3.)] {
            let key = key ^ u64::from(VERSION);
            std::fs::write(
                root.join(format!("probe-gi/{key:016x}-auto.gi")),
                encode(&lattice(origin)),
            )?;
        }
        let frame = RenderFrame::default();
        let none = BTreeSet::new();
        acquisition.request(1, &frame, || Ok(Content::default()), &none)?;
        acquisition.request(2, &frame, || Ok(Content::default()), &none)?;
        acquisition.request(3, &frame, || Ok(Content::default()), &none)?;
        // Repeating the newest key resolves no content.
        acquisition.request(3, &frame, || anyhow::bail!("resolved twice"), &none)?;
        assert!(acquisition.queued.is_some(), "requests during a bake queue");
        assert!(acquisition.is_pending());
        let surfaces = acquisition.poll(true).context("the newest request")?;
        assert_eq!(surfaces.origin[0], 3.);
        assert!(!acquisition.is_pending());
        assert!(acquisition.poll(false).is_none());
        Ok(())
    }

    /// A 24 m slab and a 2x3x2 m block at `x` (entity 2).
    fn yard(x: f32) -> Result<native::Prepared> {
        use ara_runtime::{
            components::MeshShape,
            extract::{Draw, Geometry},
        };
        let draw = |entity: u64, size: [f32; 3], at: [f32; 3]| Draw {
            entity,
            address: format!("e_{entity}"),
            label: String::new(),
            geometry: Geometry::Shape {
                shape: MeshShape::Box,
                size,
            },
            world: glam::Mat4::from_translation(glam::Vec3::from_array(at)),
            color: [0.7, 0.7, 0.7, 1.],
            roughness: 0.8,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
        };
        let mut frame = RenderFrame::default();
        frame.draws = vec![
            draw(1, [24., 0.5, 24.], [0., -0.25, 0.]),
            draw(2, [2., 3., 2.], [x, 1.5, 0.]),
        ];
        native::prepare(&frame, Content::default(), config())
    }

    fn probe(s: &ProbeSurfaces, id: usize) -> (&[[u32; 4]], &[[f32; 4]]) {
        (
            &s.texels[id * SURFACE_TEXELS..(id + 1) * SURFACE_TEXELS],
            &s.statics[id * STATIC_VECTORS..(id + 1) * STATIC_VECTORS],
        )
    }

    #[test]
    fn an_edit_retraces_the_old_spot_to_the_no_entity_bake() -> Result<()> {
        let none = BTreeSet::new();
        let before = yard(-6.)?;
        let mut tracer = Tracer::new(&before.scene, &shading(&before), &none);
        let mut surfaces = gi_cpu::bake(&tracer)?;
        let lattice = surfaces.clone();
        let all: Vec<usize> = (0..surfaces.len()).collect();
        // Fresh bakes of `scene`, traced on the same lattice.
        let fresh = |scene: &native::Prepared, dynamic: &BTreeSet<u64>| -> Result<ProbeSurfaces> {
            let tracer = Tracer::new(&scene.scene, &shading(scene), dynamic);
            let mut fresh = lattice.clone();
            gi_cpu::rebake(&tracer, &mut fresh, &all)?;
            Ok(fresh)
        };
        let stale = fresh(&before, &none)?;

        // Move the block 12 m: only probes near its old and new places re-trace,
        // and each matches a bake of the moved scene exactly.
        let moved = glam30::Mat4::from_translation(glam30::Vec3::new(6., 1.5, 0.));
        let boxes = tracer.edit(&[(2, moved)], &none);
        let probes = gi_cpu::affected(&surfaces, &boxes);
        assert!(
            !probes.is_empty() && probes.len() * 2 < surfaces.len(),
            "{} of {} probes",
            probes.len(),
            surfaces.len()
        );
        gi_cpu::rebake(&tracer, &mut surfaces, &probes)?;
        let after = yard(6.)?;
        let reference = fresh(&after, &none)?;
        for &id in &probes {
            assert!(probe(&surfaces, id) == probe(&reference, id), "probe {id}");
        }
        // The old spot really changed: some re-traced probe no longer sees the block.
        assert!(
            probes
                .iter()
                .any(|&id| probe(&stale, id) != probe(&reference, id))
        );

        // A dynamic entity leaves the surfaces entirely.
        let dynamic = BTreeSet::from([2]);
        let boxes = tracer.edit(&[], &dynamic);
        let probes = gi_cpu::affected(&surfaces, &boxes);
        gi_cpu::rebake(&tracer, &mut surfaces, &probes)?;
        let reference = fresh(&after, &dynamic)?;
        for &id in &probes {
            assert!(probe(&surfaces, id) == probe(&reference, id), "probe {id}");
        }
        Ok(())
    }

    #[test]
    fn moved_instances_stay_dynamic_until_they_rest() -> Result<()> {
        let mut prepared = yard(0.)?;
        let mut mobility = Mobility::default();
        assert!(
            !mobility.observe(&prepared.scene, true),
            "content starts at rest"
        );
        assert!(mobility.dynamic().is_empty());
        prepared.scene.instances[1].world =
            glam30::Mat4::from_translation(glam30::Vec3::new(3., 1.5, 0.));
        assert!(mobility.observe(&prepared.scene, true));
        assert_eq!(mobility.dynamic(), &BTreeSet::from([2]));
        assert!(mobility.settling());
        for _ in 1..REST_FRAMES {
            assert!(!mobility.observe(&prepared.scene, false));
        }
        assert!(mobility.observe(&prepared.scene, false), "at rest again");
        assert!(mobility.dynamic().is_empty() && !mobility.settling());
        Ok(())
    }

    #[test]
    fn cache_round_trips_and_rejects_truncation() -> Result<()> {
        let surfaces = ProbeSurfaces {
            origin: [1., 2., 3.],
            spacing: [0.5, 1., 2.],
            counts: [2, 2, 2],
            texels: (0..8 * SURFACE_TEXELS as u32)
                .map(|i| [i, i + 1, i + 2, i + 3])
                .collect(),
            statics: vec![[0.25, 1., 0., 1.]; 8 * STATIC_VECTORS],
        };
        let bytes = encode(&surfaces);
        assert_eq!(decode(&bytes)?, surfaces);
        assert!(decode(&bytes[..bytes.len() - 4]).is_err());
        Ok(())
    }
}
