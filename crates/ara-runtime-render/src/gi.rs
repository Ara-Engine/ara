//! Probe GI acquisition for the raster path. A content generation's static
//! instances are traced once into probe surfaces on the CPU (`gi_cpu`), or with
//! `ARA_PROBE_GI_RT=1` on a second, ray-tracing device where available
//! (`bake_probe_surfaces` on a temporary fidelity view); the raster renderer
//! relights them every frame without rays.
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
use ara_render::look::gi::{
    ProbeLattice, ProbeLayout, ProbeSurfaces, ProbeUpdate, STATIC_VECTORS, SURFACE_TEXELS,
};
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
const VERSION: u32 = 5;
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
    /// Small reservation metadata, before the expensive bake/payload completes.
    Layout(ProbeLayout),
    /// The job's packed probe records (delivered as soon as they exist): the
    /// re-traced probes, or every probe after a full bake. A pool's full bake is
    /// two of them: its coarsest lattice (all the volume needs to be ready), then
    /// its finer lattices with `last` set.
    Surfaces {
        update: Result<ProbeUpdate>,
        last: bool,
    },
    /// The job finished, returning the worker state.
    Done(Option<Worker>),
}

struct Active {
    /// A scene boundary invalidated this job; drain it without publishing.
    discarded: bool,
    /// The first records arrived: readiness no longer waits on this job.
    delivered: bool,
    /// The last records arrived: captures stop waiting.
    complete: bool,
    receive: Receiver<Message>,
}

/// Background probe surface acquisition for one view. At most one job runs at a
/// time; while it runs, only the newest request waits (bursts of publications or
/// edits never stack bakes). Every finished job's update is delivered, in order:
/// incremental updates build on the ones before them.
pub(crate) struct Acquisition {
    layout: Option<ProbeLayout>,
    cache: PathBuf,
    /// Trace on the CPU, the default: a second ray-tracing device contends with
    /// presentation for the driver (100+ ms submits), and the CPU bake matches
    /// its quality. `ARA_PROBE_GI_RT=1` opts into the ray-traced bake.
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
    /// Forget another scene without waiting for its worker. The next request
    /// queues behind it, preserving the single-worker budget; late deliveries and
    /// the old incremental tracer are discarded rather than lighting the new scene.
    pub(crate) fn reset_scene(&mut self) {
        self.layout = None;
        self.key = None;
        self.source = None;
        self.queued = None;
        self.worker = None;
        if let Some(active) = &mut self.active {
            active.discarded = true;
        }
    }
    #[cfg(test)]
    pub(crate) fn request_serial(&self) -> u64 {
        self.newest
    }

    pub(crate) fn new(cache: PathBuf) -> Self {
        Self::with_method(cache, std::env::var_os("ARA_PROBE_GI_RT").is_none())
    }

    /// `cpu` false bakes on a ray-tracing device where one exists (the CPU
    /// otherwise); each method caches separately.
    pub(crate) fn with_method(cache: PathBuf, cpu: bool) -> Self {
        Self {
            layout: None,
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

    /// Latest worker-published allocation dimensions; no surface payload or wait.
    pub(crate) fn layout(&self) -> Option<ProbeLayout> {
        self.layout
    }

    fn start(&mut self, job: Job) -> Result<()> {
        let (cache, cpu, worker) = (self.cache.clone(), self.cpu, self.worker.take());
        let (send, receive) = channel();
        std::thread::Builder::new()
            .name("probe-gi".into())
            .spawn(move || run(&cache, cpu, job, worker, &send))
            .context("starting the probe GI worker")?;
        self.active = Some(Active {
            discarded: false,
            delivered: false,
            complete: false,
            receive,
        });
        Ok(())
    }

    /// The next finished job's update, in order; `wait` blocks until the newest
    /// request's has arrived (call until `None`). Failures are reported and leave
    /// the last surfaces in use.
    pub(crate) fn poll(&mut self, wait: bool) -> Option<ProbeUpdate> {
        loop {
            let active = self.active.as_mut()?;
            let block = wait && (!active.complete || self.queued.is_some());
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
                Some(Message::Layout(layout)) => {
                    if !active.discarded {
                        self.layout = Some(layout);
                    }
                }
                Some(Message::Surfaces { update, last }) => {
                    active.delivered = true;
                    active.complete |= last;
                    if active.discarded {
                        continue;
                    }
                    match update {
                        Ok(update) => return Some(update),
                        Err(error) => {
                            eprintln!("ara-runtime-render: probe GI acquisition failed: {error:#}")
                        }
                    }
                }
                done => {
                    match done {
                        Some(Message::Done(worker)) if !active.discarded => self.worker = worker,
                        Some(Message::Done(_)) => {}
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
    let mut early = false;
    let follow = match acquire(cache, cpu, &job, &mut worker, send, &mut early) {
        Ok((surfaces, changed, follow)) => {
            // RT acquisition and cache hits use the same reservation contract.
            let _ = send.send(Message::Layout(surfaces.layout()));
            // Packing happens here, off the render thread.
            for (probes, last) in stages(&surfaces, changed, early) {
                let update = ProbeUpdate::new(&surfaces, probes.as_deref());
                let _ = send.send(Message::Surfaces { update, last });
            }
            note("delivered");
            follow
        }
        Err(error) => {
            let update = Err(error);
            let _ = send.send(Message::Surfaces { update, last: true });
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
            .map(|mut prepared| {
                let shading = bake_shading(&mut prepared);
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

/// What a job delivers, in order (probe ids, `None` for all; whether last): an
/// edit's re-traced probes; a single lattice whole; a pool's coarsest lattice
/// (unless the bake already sent it, `early`) and then its finer lattices.
fn stages(
    surfaces: &ProbeSurfaces,
    changed: Option<Vec<u32>>,
    early: bool,
) -> Vec<(Option<Vec<u32>>, bool)> {
    let layout = surfaces.layout();
    let finer = layout.bases()[layout.lattices().len() - 1] as u32;
    match changed {
        Some(changed) => vec![(Some(changed), true)],
        None if finer == 0 => vec![(None, true)],
        None => {
            let fine = (0..finer).collect();
            let coarse = (finer..layout.probe_count() as u32).collect();
            let mut stages = vec![(Some(coarse), false), (Some(fine), true)];
            if early {
                stages.remove(0);
            }
            stages
        }
    }
}

/// Produces the job's surfaces: an edit re-traces in place; a generation is
/// incremental against the previous tracer where the lattice still fits and
/// under half the probes change, else loaded from the cache or fully baked (on
/// a new ray-tracing device where available). Also returns the re-traced probes
/// (`None` after a full bake) and what the worker still needs for a tracer of the
/// delivered scene. A CPU bake of a pool sends its coarsest lattice as soon as it
/// is traced, before the finer ones (`early`).
fn acquire(
    cache: &Path,
    cpu: bool,
    job: &Job,
    worker: &mut Option<Worker>,
    send: &Sender<Message>,
    early: &mut bool,
) -> Result<(ProbeSurfaces, Option<Vec<u32>>, Option<FollowUp>)> {
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
        let changed = probes.iter().map(|&p| p as u32).collect();
        return Ok((state.surfaces.clone(), Some(changed), None));
    }
    let method = if cpu { "cpu" } else { "auto" };
    // The lattice rule names the file: a changed rule misses older bakes.
    let path = cache.join(format!(
        "{:016x}-{method}-{}.gi",
        job.key ^ dynamic_key(&job.dynamic),
        gi_cpu::lattice_rule()
    ));
    let mut tracer = worker.as_mut().and_then(|w| w.tracer.take());
    if let Ok(bytes) = std::fs::read(&path) {
        match decode(&bytes) {
            Ok(surfaces) => {
                let _ = send.send(Message::Layout(surfaces.layout()));
                *worker = Some(Worker {
                    key: job.key,
                    surfaces: surfaces.clone(),
                    tracer: None,
                });
                return Ok((
                    surfaces,
                    None,
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
    let shading = bake_shading(&mut prepared);
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
            let changed = probes.iter().map(|&p| p as u32).collect();
            return Ok((state.surfaces.clone(), Some(changed), None));
        }
        eprintln!(
            "ara-runtime-render: full probe bake for new content: {}",
            if fits {
                format!(
                    "{} of {} probes changed",
                    probes.len(),
                    state.surfaces.len()
                )
            } else {
                "it outgrew the probe lattice".into()
            }
        );
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
            let tracer =
                tracer.get_or_insert_with(|| Tracer::new(&prepared.scene, &shading, &job.dynamic));
            let surfaces = bake(&mut gpu, &job.frame, &prepared, tracer, |layout| {
                let _ = send.send(Message::Layout(layout));
            });
            prepared.scene.instances.extend(excluded);
            surfaces?
        }
        None => {
            let tracer =
                tracer.get_or_insert_with(|| Tracer::new(&prepared.scene, &shading, &job.dynamic));
            gi_cpu::bake_with_layout(
                tracer,
                |layout| {
                    let _ = send.send(Message::Layout(layout));
                },
                |coarse| {
                    let (probes, _) = stages(coarse, None, false).remove(0);
                    let update = ProbeUpdate::new(coarse, probes.as_deref());
                    let _ = send.send(Message::Surfaces {
                        update,
                        last: false,
                    });
                    *early = true;
                },
            )?
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
    Ok((surfaces, None, follow))
}

/// Cache file: [`MAGIC`], the lattice count, nine words per lattice (origin and
/// spacing as f32, counts as u32), then the texels and statics of every probe.
fn encode(s: &ProbeSurfaces) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend((s.layout.lattices().len() as u32).to_le_bytes());
    for lattice in s.layout.lattices() {
        for v in lattice.origin.iter().chain(&lattice.spacing) {
            bytes.extend(v.to_le_bytes());
        }
        for c in lattice.counts {
            bytes.extend(c.to_le_bytes());
        }
    }
    bytes.extend_from_slice(bytemuck::cast_slice(&s.texels));
    bytes.extend_from_slice(bytemuck::cast_slice(&s.statics));
    bytes
}

fn decode(bytes: &[u8]) -> Result<ProbeSurfaces> {
    ensure!(
        bytes.len() >= 12 && &bytes[..8] == MAGIC,
        "not a probe GI cache"
    );
    let word = |i: usize| -> Result<[u8; 4]> {
        bytes
            .get(8 + i * 4..12 + i * 4)
            .and_then(|w| w.try_into().ok())
            .context("truncated probe GI cache")
    };
    let lattices = u32::from_le_bytes(word(0)?) as usize;
    ensure!(
        (1..=ara_render::look::gi::MAX_LATTICES).contains(&lattices),
        "invalid probe GI cache lattices"
    );
    let mut all = Vec::with_capacity(lattices);
    for l in 0..lattices {
        let at = 1 + l * 9;
        let float = |i: usize| -> Result<f32> { Ok(f32::from_le_bytes(word(at + i)?)) };
        let count = |i: usize| -> Result<u32> { Ok(u32::from_le_bytes(word(at + i)?)) };
        all.push(ProbeLattice {
            origin: [float(0)?, float(1)?, float(2)?],
            spacing: [float(3)?, float(4)?, float(5)?],
            counts: [count(6)?, count(7)?, count(8)?],
        });
    }
    let layout = ProbeLayout::new(&all)?;
    layout.validate()?;
    let header = 12 + lattices * 36;
    let probes = layout.probe_count();
    let texel_bytes = probes * SURFACE_TEXELS * 16;
    ensure!(
        bytes.len() == header + texel_bytes + probes * STATIC_VECTORS * 16,
        "truncated probe GI cache"
    );
    let surfaces = ProbeSurfaces {
        layout,
        texels: bytemuck::pod_collect_to_vec(&bytes[header..header + texel_bytes]),
        statics: bytemuck::pod_collect_to_vec(&bytes[header + texel_bytes..]),
    };
    surfaces.validate()?;
    Ok(surfaces)
}

/// The shading a bake traces: [`shading`] with emitter materials' emission
/// removed, here and in `prepared`'s materials (the ray-traced bake reads those).
/// Emitters light the scene as direct lights (`emitters`); probes keep their bounce.
fn bake_shading(prepared: &mut native::Prepared) -> Vec<crate::gi_cpu::Shading> {
    let mut shading = shading(prepared);
    for (material, s) in prepared.scene.materials.iter_mut().zip(&mut shading) {
        if crate::emitters::is_emitter(*s) {
            s.emissive = glam30::Vec3::ZERO;
            material.pbr.emissive_roughness[..3].fill(0.);
        }
    }
    shading
}

/// Per-material diffuse albedo, emission and cutout for the CPU bake, from the
/// materials' factors and their maps' CPU means (terrain splats average their
/// layers here; the tracer blends them per hit).
pub(crate) fn shading(prepared: &native::Prepared) -> Vec<crate::gi_cpu::Shading> {
    shade(prepared, true)
}

/// Emission only (albedo and cutout left zero): what the emitter search snapshots
/// on the render thread, without decoding colour or alpha maps.
pub(crate) fn emission(prepared: &native::Prepared) -> Vec<crate::gi_cpu::Shading> {
    shade(prepared, false)
}

/// [`shading`] (`surface`) or [`emission`]. Each map's mean and each cutout's
/// coverage is decoded once per call, however many materials share the map.
fn shade(prepared: &native::Prepared, surface: bool) -> Vec<crate::gi_cpu::Shading> {
    use ara_render::{
        instanced::NO_TEXTURE,
        scene::{BASE_COLOR_DECODE_SRGB, EMISSIVE_DECODE_SRGB},
    };
    use glam30::Vec3;
    let mut missing = 0usize;
    let mut means = HashMap::new();
    // `decode`: the material decodes this raw map's sRGB itself (graph views).
    let mut mean = |id: u32, decode: bool| {
        if id == NO_TEXTURE {
            return [1.; 4];
        }
        *means.entry((id, decode)).or_insert_with(|| {
            prepared.textures.mean_texel(id, decode).unwrap_or_else(|| {
                missing += 1;
                [1.; 4]
            })
        })
    };
    // Share of rays an alpha-tested (mode 1) or blended (mode 2) material lets
    // through, as the ray-traced bake's any-hit test (`nativeAccept`) decides:
    // over the base colour's texels at up to CUTOUT_SIDE a side, where thin
    // features (fence wire) still hold their coverage.
    const CUTOUT_SIDE: u32 = 256;
    let mut cutouts = HashMap::new();
    let mut cutout = |m: &ara_render::instanced::Material| {
        let (mode, alpha, cutoff) = (m.pbr.flags[0], m.pbr.base_color[3], m.pbr.surface[3]);
        if mode == 0 {
            return 0.;
        }
        let key = (m.pbr.textures[0], mode, alpha.to_bits(), cutoff.to_bits());
        *cutouts.entry(key).or_insert_with(|| {
            let alphas: Vec<f32> = match m.pbr.textures[0] {
                NO_TEXTURE => vec![alpha],
                id => prepared
                    .textures
                    .texels(id, CUTOUT_SIDE, false)
                    .map_or_else(
                        || vec![alpha],
                        |(_, t)| t.iter().map(|t| alpha * t[3]).collect(),
                    ),
            };
            let n = alphas.len().max(1) as f32;
            let kept = if mode == 1 {
                alphas.iter().filter(|&&a| a >= cutoff).count() as f32 / n
            } else {
                alphas.iter().sum::<f32>() / n
            };
            (1. - kept).clamp(0., 1.)
        })
    };
    let materials = &prepared.scene.materials;
    let own: Vec<_> = materials
        .iter()
        .map(|m| {
            let (c, e) = (m.pbr.base_color, m.pbr.emissive_roughness);
            let decode = |bit: u32| m.pbr.flags[1] & bit != 0;
            let et = mean(m.pbr.textures[3], decode(EMISSIVE_DECODE_SRGB));
            let emissive = Vec3::new(e[0] * et[0], e[1] * et[1], e[2] * et[2]);
            if !surface {
                return crate::gi_cpu::Shading {
                    emissive,
                    ..Default::default()
                };
            }
            let t = mean(m.pbr.textures[0], decode(BASE_COLOR_DECODE_SRGB));
            // glTF metalness lives in the blue channel of the metal/roughness map.
            let metal = (m.pbr.surface[0] * mean(m.pbr.textures[2], false)[2]).clamp(0., 1.);
            crate::gi_cpu::Shading {
                albedo: Vec3::new(c[0] * t[0], c[1] * t[1], c[2] * t[2]) * (1. - metal),
                emissive,
                cutout: cutout(m),
            }
        })
        .collect();
    if missing > 0 {
        eprintln!(
            "ara-runtime-render: CPU probe bake: {missing} maps have no CPU mean (taken as white)"
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
                ..shading
            }
        })
        .collect()
}

/// Bakes the probe surfaces of `frame`'s content on a ray-tracing device: the
/// fidelity view's lattice is traced there; a fine lattice over built-up content
/// (`gi_cpu::layout`) bakes on `tracer`. The final pool is published before
/// either traces.
fn bake(
    gpu: &mut VulkanDevice,
    frame: &RenderFrame,
    prepared: &native::Prepared,
    tracer: &Tracer,
    publish: impl FnOnce(ProbeLayout),
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
    let surfaces = (|| -> Result<ProbeSurfaces> {
        let coarse = view.probe_volume();
        let coarse = ProbeLattice {
            origin: coarse.origin.to_array(),
            spacing: coarse.spacing.to_array(),
            counts: coarse.counts,
        };
        let cpu = gi_cpu::layout(tracer)?;
        let layout = match cpu.lattices() {
            [fine, _] => ProbeLayout::new(&[*fine, coarse])?,
            _ => ProbeLayout::single(coarse),
        };
        publish(layout);
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
        let traced = view.bake_probe_surfaces(gpu)?;
        if layout.lattices().len() == 1 {
            return Ok(traced);
        }
        // The fine lattice from the CPU tracer; the coarse one as ray-traced.
        let mut pool = gi_cpu::bake_lattices(tracer, layout, 0..1)?;
        let base = layout.bases()[1];
        pool.texels[base * SURFACE_TEXELS..].copy_from_slice(&traced.texels);
        pool.statics[base * STATIC_VECTORS..].copy_from_slice(&traced.statics);
        pool.validate()?;
        Ok(pool)
    })();
    view.destroy(gpu)?;
    let surfaces = surfaces?;
    ensure!(!surfaces.is_empty(), "empty probe bake");
    let counts: Vec<_> = surfaces
        .layout
        .lattices()
        .iter()
        .map(|l| l.counts)
        .collect();
    eprintln!(
        "ara-runtime-render: baked {} GI probes ({counts:?}) in {:.1} s",
        surfaces.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(surfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice(origin: f32) -> ProbeSurfaces {
        ProbeSurfaces {
            layout: ProbeLayout::single(ProbeLattice {
                origin: [origin, 0., 0.],
                spacing: [1.; 3],
                counts: [2, 2, 2],
            }),
            texels: vec![[0; 4]; 8 * SURFACE_TEXELS],
            statics: vec![[1., 1., 0., 1.]; 8 * STATIC_VECTORS],
        }
    }

    #[test]
    fn scene_reset_discards_in_flight_surfaces_and_retraces_reused_content_keys() -> Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/runtime-render/probe-scene-reset")
            .join(std::process::id().to_string());
        let cache = root.join("probe-gi");
        std::fs::create_dir_all(&cache)?;
        let key = 7u64 ^ u64::from(VERSION);
        std::fs::write(
            cache.join(format!("{key:016x}-cpu-{}.gi", gi_cpu::lattice_rule())),
            encode(&lattice(7.)),
        )?;
        let mut acquisition = Acquisition::new(root);
        let frame = RenderFrame::default();
        let dynamic = BTreeSet::new();
        acquisition.request(7, &frame, || Ok(Content::default()), &dynamic)?;
        // Even an already-finished worker has not been adopted until poll. A
        // scene boundary must drop its layout, surfaces and incremental tracer.
        acquisition.reset_scene();
        assert!(acquisition.layout().is_none());
        // An identical content key in another scene still starts an acquisition.
        acquisition.request(7, &frame, || Ok(Content::default()), &dynamic)?;
        assert_eq!(acquisition.request_serial(), 2);
        assert!(
            acquisition.queued.is_some(),
            "one worker across scene boundaries"
        );
        let mut origins = Vec::new();
        while let Some(update) = acquisition.poll(true) {
            origins.push(update.layout.lattices()[0].origin[0]);
        }
        assert_eq!(origins, [7.], "only the replacement scene may publish");
        assert!(!acquisition.is_pending());
        Ok(())
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
                root.join(format!(
                    "probe-gi/{key:016x}-cpu-{}.gi",
                    gi_cpu::lattice_rule()
                )),
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
        // Every finished job delivers, in order; the newest arrives last.
        let mut origins = Vec::new();
        while let Some(update) = acquisition.poll(true) {
            origins.push(update.layout.lattices()[0].origin[0]);
        }
        assert_eq!(origins.last(), Some(&3.), "{origins:?}");
        assert!(!origins.contains(&2.), "the replaced request never ran");
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

    /// A constant BC4 map (red `value`/255) registered in `prepared`'s residency.
    fn constant_map(prepared: &mut native::Prepared, value: u8) -> Result<u32> {
        use ara_asset_format::{TextureFormat, write_ktx2};
        let mut level = vec![0u8; 4 * 8];
        for block in level.as_chunks_mut::<8>().0 {
            block[..2].fill(value);
        }
        let ktx2 = write_ktx2(8, 8, TextureFormat::Bc4, &[level])?;
        let source = ara_texture_stream::NativeSource::from_cooked(ktx2.into())?;
        Ok(prepared
            .textures
            .register([value; 32], std::sync::Arc::new(source))?
            .index())
    }

    #[test]
    fn emission_is_shadings_emission_without_its_surface() -> Result<()> {
        use ara_render::{instanced::NO_TEXTURE, scene::EMISSIVE_DECODE_SRGB};
        let mut prepared = yard(0.)?;
        // One emissive map read raw, then by a material that decodes its sRGB
        // itself (a graph view), and a terrain splat layering those two.
        let map = constant_map(&mut prepared, 200)?;
        let base = prepared.scene.materials[0];
        let emitting = |decode: bool| {
            let mut m = base;
            m.pbr.textures[3] = map;
            m.pbr.emissive_roughness[..3].copy_from_slice(&[4., 2., 1.]);
            m.pbr.flags[1] = if decode { EMISSIVE_DECODE_SRGB } else { 0 };
            m
        };
        let first = prepared.scene.materials.len();
        let mut terrain = base;
        terrain.extras.terrain_layers = 2;
        terrain.extras.layer_materials = [first as u32, first as u32 + 1, NO_TEXTURE, NO_TEXTURE];
        prepared
            .scene
            .materials
            .extend([emitting(false), emitting(true), terrain]);
        let (full, emitted) = (shading(&prepared), emission(&prepared));
        assert_eq!(full.len(), emitted.len());
        for (f, e) in full.iter().zip(&emitted) {
            assert_eq!(f.emissive, e.emissive);
            assert_eq!((e.albedo, e.cutout), (glam30::Vec3::ZERO, 0.));
        }
        assert!(full.iter().any(|f| f.albedo != glam30::Vec3::ZERO));
        let [raw, decoded, splat] = [first, first + 1, first + 2].map(|i| emitted[i].emissive.x);
        let linear = ((200. / 255. + 0.055) / 1.055f32).powf(2.4);
        assert!((raw - 4. * 200. / 255.).abs() < 1e-3, "{raw}");
        assert!((decoded - 4. * linear).abs() < 1e-3, "{decoded}");
        assert!((splat - (raw + decoded) / 2.).abs() < 1e-4, "{splat}");
        Ok(())
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
        // A two-lattice pool: eight fine probes, then twelve coarse ones.
        let surfaces = ProbeSurfaces {
            layout: ProbeLayout::new(&[
                ProbeLattice {
                    origin: [1., 2., 3.],
                    spacing: [0.5, 1., 2.],
                    counts: [2, 2, 2],
                },
                ProbeLattice {
                    origin: [-4., 0., -4.],
                    spacing: [4., 4., 4.],
                    counts: [3, 2, 2],
                },
            ])?,
            texels: (0..20 * SURFACE_TEXELS as u32)
                .map(|i| [i, i + 1, i + 2, i + 3])
                .collect(),
            statics: vec![[0.25, 1., 0., 1.]; 20 * STATIC_VECTORS],
        };
        let bytes = encode(&surfaces);
        assert_eq!(decode(&bytes)?, surfaces);
        assert!(decode(&bytes[..bytes.len() - 4]).is_err());
        // A full delivery of the pool is staged: the coarse lattice's twelve
        // probes (all readiness needs), then the eight fine ones, last.
        let (coarse, fine) = ((8..20).collect(), (0..8).collect());
        assert_eq!(
            stages(&surfaces, None, false),
            [(Some(coarse), false), (Some(fine), true)]
        );
        // A CPU bake already sent the coarse lattice; edits stay one stage.
        assert_eq!(
            stages(&surfaces, None, true),
            [(Some((0..8).collect()), true)]
        );
        assert_eq!(
            stages(&surfaces, Some(vec![3, 9]), false),
            [(Some(vec![3, 9]), true)]
        );
        let single = ProbeSurfaces {
            layout: ProbeLayout::single(surfaces.layout.lattices()[1]),
            texels: surfaces.texels[8 * SURFACE_TEXELS..].to_vec(),
            statics: surfaces.statics[8 * STATIC_VECTORS..].to_vec(),
        };
        assert_eq!(stages(&single, None, false), [(None, true)]);
        Ok(())
    }
}
