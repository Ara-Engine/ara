//! Drives Renderlab's `Engine` from extracted frames and hands each image to the
//! presenter's device. Renderlab renders on its own device: where the two devices
//! can share images the frame crosses through a shared ring (`shared`), otherwise by
//! a CPU copy (a readback there, an upload here). Either crossing is timed.
use crate::{
    ActivePath, Options, PathChoice,
    decals::Decals,
    effects::Effects,
    gaps::Gaps,
    lighting::{self, Metering},
    meter::Readbacks,
    scene::{MAX_GROUPS, Scene},
    shared::SharedHandoff,
    sky::Skies,
    terrain::Terrain,
};
use anyhow::{Context, Result, anyhow};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk,
};
use ara_runtime_render::{EXTERNAL_PASSES, ExternalFrame, ExternalView};
use renderlab::{
    ViewId,
    engine::{
        Capabilities, Engine, EngineDescriptor, ExperimentalRayQuery, Light, Limits, RenderPath,
        Sun, ViewDesc, ViewLook, instance_flags,
    },
};
use std::time::{Duration, Instant};

/// Longest a present waits for its frame before reading it back regardless.
const FRAME_WAIT: Duration = Duration::from_secs(5);
/// How long a click waits for the engine's id readback.
const PICK_WAIT: Duration = Duration::from_millis(250);
/// Mesh-material groups terrain tiles may take; each resident tile is its own group (its own
/// weights texture, so tiles never merge by material). Outpost's 144 tiles need 144; entity
/// draws and foliage get `MAX_GROUPS - TERRAIN_GROUPS`, merged by material when they exceed it.
const TERRAIN_GROUPS: usize = MAX_GROUPS * 3 / 4;

/// The presenter-device image each frame lands in, and how it gets there.
struct Target {
    image: Image,
    size: [u32; 2],
    /// Staging for the CPU copy, allocated on its first use.
    staging: Option<Buffer>,
    /// The image holds no frame yet (its first barrier starts from `Undefined`).
    fresh: bool,
    /// The shared ring, when this size's frames cross without a CPU copy.
    shared: Option<SharedHandoff>,
    /// Packed ids retained on the presenter, beside `image`.
    id: Option<crate::shared::PresentedId>,
    /// The texture Renderlab writes the view's ids into (`Options::scene_ids`).
    ids: Option<crate::id_target::IdTarget>,
}

/// Most frames the bridge can keep in flight: the CPU records and submits the next while the GPU
/// draws the last, so a frame costs the longer of the two, not their sum. Two is enough; a third
/// would only add latency.
const MAX_IN_FLIGHT: usize = 2;

/// `RENDERLAB_SUBMIT_LOG` (the one Renderlab reads for its submit timings): `1` prints a line to
/// stderr, any other value is a file lines are appended to. The bridge adds one line per frame
/// taken: how long `begin` took, when the GPU's completion was first seen, the time in flight
/// and the handoff's.
fn log_line(line: impl FnOnce() -> String) {
    static TARGET: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let Some(target) = TARGET
        .get_or_init(|| std::env::var("RENDERLAB_SUBMIT_LOG").ok().filter(|v| !v.is_empty()))
        .as_deref()
    else {
        return;
    };
    let line = line();
    if target == "1" {
        eprintln!("{line}");
    } else if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(target)
    {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

/// A frame begun and not yet taken.
struct Pending {
    frame: renderlab::engine::FrameId,
    view: ViewId,
    since: Instant,
    /// How long `begin` took to record and submit it.
    begin_ms: f64,
    /// The engine reported it complete.
    done: bool,
    /// Milliseconds from `since` to the poll that first saw it complete.
    done_after_ms: Option<f64>,
    /// Shared handoff: the frame's copy into the shared slot is queued. The slot holds one frame
    /// at a time, so a frame begun while the last one is still undelivered is published once
    /// that one is delivered (`Bridge::finish`), before any later frame is recorded.
    published: bool,
    /// CPU handoff: the colour and id copies queued behind the frame when it was recorded.
    readback: Option<(
        crate::id_target::Readback,
        Option<crate::id_target::Readback>,
    )>,
}

/// Lighting last applied to the engine, so unchanged frames set nothing.
#[derive(Default)]
struct Applied {
    sun: Option<Sun>,
    lights: Vec<Light>,
    /// The colour grade baked into Renderlab's lookup table (`grade::key`).
    grade: Option<u64>,
}

pub(crate) struct Bridge {
    /// Declared before `engine`: the shared ring lives on the engine's device and
    /// must drop first.
    target: Option<Target>,
    engine: Engine,
    path: ActivePath,
    scene: Scene,
    terrain: Terrain,
    decals: Decals,
    effects: Effects,
    gaps: Gaps,
    applied: Applied,
    metering: Metering,
    /// The one view the bridge renders (the presenter's, view 0), created on the first frame.
    view: Option<ViewId>,
    /// What the view is set to: its size and look (its colour table is baked, `Applied::grade`).
    desc: ViewDesc,
    /// The look last applied to the view without its table, so unchanged frames set nothing.
    last_look: Option<ViewLook>,
    /// The GPU histogram passes in flight for the meter.
    readbacks: Readbacks,
    max_reflection_roughness: Option<f32>,
    skies: Skies,
    /// The frames cross by CPU copy: asked for, or the shared route is unavailable.
    cpu_copy: bool,
    /// Ids are written beside every frame (`Options::scene_ids`).
    scene_ids: bool,
    gpu_ms: Option<f64>,
    /// Renderlab's own passes for the frame `gpu_ms` measured (`ExternalFrame::passes`).
    passes: [(&'static str, f64); EXTERNAL_PASSES],
    cut: bool,
    /// How many frames `begin` allows in flight (`Options::frames_in_flight`, at most
    /// `MAX_IN_FLIGHT`).
    in_flight_limit: usize,
    /// The frames begun and not yet taken, oldest first (`Bridge::begin`, `Bridge::finish`).
    pending: std::collections::VecDeque<Pending>,
    /// The last frame's eye and view direction, to notice a view that jumps.
    last_view: Option<([f32; 3], [f32; 3])>,
}

/// A Vulkan adapter for the hosted engine on an instance whose validation follows Ara's one
/// switch: `ARA_NO_GPU_VALIDATION` set non-empty turns it off (Renderlab's own device
/// otherwise validates in debug builds whatever Ara's device does).
fn adapter() -> Result<wgpu::Adapter> {
    let mut flags = instance_flags();
    if std::env::var_os("ARA_NO_GPU_VALIDATION").is_some_and(|v| !v.is_empty()) {
        flags = flags.difference(
            wgpu::InstanceFlags::VALIDATION
                | wgpu::InstanceFlags::DEBUG
                | wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL,
        );
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let request = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    });
    // The request resolves without waiting on anything but the driver.
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::sync::Arc::new(Unpark(std::thread::current())).into();
    let mut context = std::task::Context::from_waker(&waker);
    let mut request = std::pin::pin!(request);
    loop {
        match request.as_mut().poll(&mut context) {
            std::task::Poll::Ready(adapter) => return Ok(adapter?),
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

impl Bridge {
    pub fn new(options: &Options) -> Result<Self> {
        let adapter = adapter().map_err(|error| {
            anyhow!("ARA_RENDERLAB_UNAVAILABLE: Renderlab found no Vulkan adapter: {error}")
        })?;
        let capabilities = Capabilities::query(&adapter);
        let path = match options.path {
            PathChoice::Auto => capabilities.preferred(),
            PathChoice::HardwareRt => Some(RenderPath::HardwareRt),
            PathChoice::NonRt => Some(RenderPath::NonRt),
        }
        .filter(|path| capabilities.supports(*path))
        .with_context(|| {
            format!(
                "ARA_RENDERLAB_UNAVAILABLE: `{}` lacks the features Renderlab's {:?} path needs (hardware RT: {}, non-RT: {})",
                capabilities.adapter.name,
                options.path,
                capabilities.hardware_rt,
                capabilities.non_rt
            )
        })?;
        if path == RenderPath::NonRt {
            eprintln!(
                "ARA_RENDERLAB_NONRT_PATENT: Renderlab's non-RT sun shadows use a method with an open patent question (Renderlab docs/patent-via-8471853.md); do not ship this path without legal review"
            );
        }
        let engine = Engine::new(
            &adapter,
            EngineDescriptor {
                path,
                // SAFETY: wgpu's ray queries are experimental (incomplete validation).
                // Ara accepts that for this device: only Renderlab's own renderer, which
                // was developed and measured on this contract, issues ray queries on it.
                experimental_ray_query: (path == RenderPath::HardwareRt)
                    .then(|| unsafe { ExperimentalRayQuery::acknowledge() }),
                pipeline_cache: options.pipeline_cache.clone(),
                // Renderlab defaults to none: ground textures smear at grazing angles.
                texture_anisotropy: 16,
                // Objects per mesh-material pair: scattered foliage shares one pair.
                limits: Limits {
                    max_instances: 65_536,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .map_err(|error| {
            anyhow!("ARA_RENDERLAB_UNAVAILABLE: Renderlab could not start: {error}")
        })?;
        let path = match path {
            RenderPath::HardwareRt => ActivePath::HardwareRt,
            RenderPath::NonRt => ActivePath::NonRt,
        };
        eprintln!(
            "Ara presentation: Renderlab ({path:?}) on `{}`",
            capabilities.adapter.name
        );
        // The meter's pipeline and buffers, made now rather than on the first metered frame.
        let readbacks = Readbacks::new(engine.device());
        Ok(Self {
            target: None,
            engine,
            path,
            scene: Scene::default(),
            terrain: Terrain::default(),
            decals: Decals::default(),
            effects: Effects::default(),
            gaps: Gaps::default(),
            applied: Applied::default(),
            metering: Metering::new(),
            view: None,
            desc: {
                let mut desc = ViewDesc::new(1, 1);
                desc.outputs.ids = options.scene_ids;
                desc
            },
            last_look: None,
            readbacks,
            max_reflection_roughness: options.max_reflection_roughness,
            skies: Skies::new(options.constant_sky),
            cpu_copy: options.cpu_copy_handoff,
            scene_ids: options.scene_ids,
            gpu_ms: None,
            passes: [("", 0.); EXTERNAL_PASSES],
            cut: true,
            in_flight_limit: std::env::var("ARA_RENDERLAB_FRAMES_IN_FLIGHT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(options.frames_in_flight)
                .clamp(1, MAX_IN_FLIGHT),
            pending: std::collections::VecDeque::new(),
            last_view: None,
        })
    }

    pub fn path(&self) -> ActivePath {
        self.path
    }

    pub fn cut(&mut self) {
        self.cut = true;
    }

    /// The frontmost id under `pixel` of the view's last frame (`Engine::request_ids`), waiting
    /// briefly for the copy: a click's answer, glass included. `None` without ids, before a
    /// frame, when all the request slots are busy or when the copy does not land in time.
    pub fn pick_id(&mut self, pixel: [u32; 2]) -> Result<Option<u32>> {
        let (Some(view), true) = (self.view, self.scene_ids) else {
            return Ok(None);
        };
        let Ok(request) = self
            .engine
            .request_ids(view, renderlab::IdRect::pixel(pixel[0], pixel[1]))
        else {
            return Ok(None);
        };
        let started = Instant::now();
        while started.elapsed() < PICK_WAIT {
            let events = self.pump()?;
            if let Some(read) = events.ids.iter().find(|read| read.request == request) {
                return Ok(Some(read.at(0, 0).front));
            }
            if events.ids_lost.iter().any(|lost| lost.request == request) {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(None)
    }

    /// Draws `view` and waits for it: [`Self::begin`], then [`Self::finish`] until it (and any
    /// frame begun before it) is done.
    pub fn render(
        &mut self,
        gpu: &mut GpuDevice,
        view: &ExternalView<'_>,
    ) -> Result<ExternalFrame> {
        while self.pending.len() >= self.in_flight_limit {
            self.finish(gpu)?;
            std::thread::yield_now();
        }
        self.begin(gpu, view)?;
        loop {
            if let Some(frame) = self.finish(gpu)?
                && self.pending.is_empty()
            {
                return Ok(frame);
            }
            std::thread::yield_now();
        }
    }

    /// Whether a frame was begun and not yet taken by [`Self::finish`].
    pub fn in_flight(&self) -> bool {
        !self.pending.is_empty()
    }

    /// How many frames were begun and not yet taken.
    pub fn frames_in_flight(&self) -> usize {
        self.pending.len()
    }

    /// How many frames [`Self::begin`] allows in flight.
    pub fn max_frames_in_flight(&self) -> usize {
        self.in_flight_limit
    }

    /// Starts drawing `view` and returns without waiting for the GPU: the scene is synced and the
    /// frame, its ids and the meter's pass are submitted, and its handoff queued behind it.
    /// [`Self::finish`] takes the oldest result. Up to [`MAX_IN_FLIGHT`] frames are in flight (the
    /// CPU records the next while the GPU draws the last); it is an error to begin another
    /// before one is taken.
    pub fn begin(&mut self, gpu: &mut GpuDevice, view: &ExternalView<'_>) -> Result<()> {
        let begin_started = Instant::now();
        anyhow::ensure!(
            self.pending.len() < self.in_flight_limit,
            "{} frames are in flight: finish one before beginning another",
            self.in_flight_limit
        );
        let frame = view.frame;
        // The meter's readings of earlier frames, the previous one's included: the bridge
        // waited for that frame and its histogram pass is queued right behind it, so the
        // reading is done by now. They are taken before this frame's exposure is chosen, so
        // a cut's reading is applied one frame later, as it was when the meter read the
        // frame back. Readings that finish while automatic exposure is off are dropped, not
        // kept for when it comes back on.
        let automatic = view.look.ev100.is_none();
        for reading in self.readbacks.poll(self.engine.device(), &mut self.gaps) {
            if automatic {
                self.metering
                    .observe(view.look, reading.meter, reading.stops, &reading.histogram);
            }
        }
        self.report_unsupported(view);
        // Terrain tiles take at most three quarters of the mesh-material groups
        // (`TERRAIN_GROUPS`); entity draws and foliage share the rest. On the non-RT path foliage is capped by the largest
        // mesh uploaded: when this frame uploads a larger one (a new cut or batch), the
        // foliage is rebuilt for it before the frame renders.
        let nonrt = self.path == ActivePath::NonRt;
        for _ in 0..2 {
            let known = self.scene.largest_triangles();
            let (tiles, assumed) = self.terrain.sync(
                &mut self.engine,
                view,
                TERRAIN_GROUPS,
                nonrt.then_some(known),
                &mut self.gaps,
            )?;
            let (foliage, foliage_key) = self.terrain.foliage();
            self.scene.sync(
                &mut self.engine,
                frame,
                foliage,
                foliage_key,
                tiles,
                &mut self.gaps,
            )?;
            if !nonrt || self.scene.largest_triangles() <= assumed {
                break;
            }
        }
        // Terrain tiles follow the frame's draws in the editor's id numbering.
        self.terrain
            .tag(&mut self.engine, frame.draws.len() as u32 + 1)?;
        self.decals.sync(&mut self.engine, frame, &mut self.gaps)?;
        self.effects.sync(&mut self.engine, frame, &mut self.gaps)?;

        let (stops, metered) = lighting::exposure_stops(view, &mut self.metering);
        let (settings, look) =
            lighting::settings(view, stops, self.max_reflection_roughness, &mut self.gaps);
        self.engine.set_settings(settings)?;
        let authored = crate::grade::authored(view);
        let grade = crate::grade::key(view.look, authored);
        let regrade = self.applied.grade != Some(grade);
        if regrade {
            self.desc.look.colour_lut = crate::grade::lut(view.look, authored);
            self.applied.grade = Some(grade);
        }
        // The view: created on the first frame; a new size or look sets it (a size restarts its
        // histories, as a resize did before views).
        if self.view.is_none()
            || regrade
            || self.last_look.as_ref() != Some(&look)
            || (self.desc.width, self.desc.height) != (view.size[0], view.size[1])
        {
            self.desc.look = ViewLook {
                colour_lut: self.desc.look.colour_lut.take(),
                ..look.clone()
            };
            (self.desc.width, self.desc.height) = (view.size[0], view.size[1]);
            match self.view {
                Some(id) => self.engine.set_view(id, &self.desc)?,
                None => self.view = Some(self.engine.create_view(&self.desc)?),
            }
            self.last_look = Some(look);
        }
        let view_id = self.view.context("the view was created above")?;
        self.skies
            .apply(&mut self.engine, view, &mut self.gaps, self.cut)?;
        let sun = lighting::sun(frame);
        if self.applied.sun != sun {
            self.engine.set_sun(sun)?;
            self.applied.sun = sun;
        }
        let lights = lighting::lights(
            frame.lights.iter().chain(self.effects.lights()),
            &mut self.gaps,
        );
        if self.applied.lights != lights {
            self.engine.set_lights(&lights)?;
            self.applied.lights = lights;
        }

        // A view that jumps (a camera cut, a teleport) shares nothing with the history
        // Renderlab accumulated: it starts over, as before a capture.
        if let Some(camera) = frame.camera {
            let view = (camera.position, camera.forward);
            self.cut |= self.last_view.is_some_and(|last| jumped(last, view));
            self.last_view = Some(view);
        }
        if std::mem::take(&mut self.cut) {
            self.engine.cut_view(view_id);
            // As Ara resets its exposure history: the meter snaps to its target.
            self.metering.snap();
            // Readings in flight describe the old view.
            self.readbacks.discard();
        }
        self.ensure_target(gpu, view.size)?;
        let rendered = self
            .engine
            .render_view(view_id, &lighting::camera(frame)?)?
            .frame;
        let target = self.target.as_mut().context("target ensured above")?;
        if self.scene_ids {
            if target.ids.as_ref().is_none_or(|t| t.size() != view.size) {
                target.ids = Some(crate::id_target::IdTarget::new(
                    self.engine.device(),
                    view.size,
                ));
            }
            if let Some(ids) = &target.ids {
                ids.write(&mut self.engine, view_id)?;
            }
        }
        let mut published = false;
        let mut readback = None;
        if let Some(shared) = &mut target.shared {
            match &target.ids {
                Some(ids) => shared.set_id(ids.texture().clone()),
                None if crate::shared::scene_ids_pattern() => {
                    shared.stage_pattern(&self.engine)?;
                }
                None => {}
            }
            // The copy into the shared slot queues behind the frame right away, unless the slot
            // still holds the frame before: that one's delivery publishes this one
            // (`Self::finish`), before any later frame is recorded over the output.
            if shared.idle() {
                shared.publish(&self.engine, view_id)?;
                published = true;
            }
        } else if let Some(output) = self.engine.view_output(view_id) {
            // The CPU copy: the frame's texels are copied into buffers now, behind it, so a frame
            // recorded after it cannot change what this one's `finish` reads.
            let colour = crate::id_target::Readback::queue(
                self.engine.device(),
                self.engine.queue(),
                output,
                view.size,
            );
            let ids = target.ids.as_ref().map(|ids| ids.queue_read(&self.engine));
            readback = Some((colour, ids));
        }

        // The meter: queue this frame's histogram pass when the meter asks (every frame while
        // it snaps, then every 16th). It does not wait; a refused pass is asked for again
        // next frame.
        if let Some(meter) = metered
            && self.metering.due()
        {
            let queued = self.engine.view_output(view_id).is_some_and(|output| {
                self.readbacks.submit(
                    self.engine.device(),
                    self.engine.queue(),
                    output,
                    meter,
                    stops,
                    &mut self.gaps,
                )
            });
            if !queued {
                self.metering.retry();
            }
        }

        self.pending.push_back(Pending {
            frame: rendered,
            view: view_id,
            since: Instant::now(),
            begin_ms: begin_started.elapsed().as_secs_f64() * 1000.,
            done: false,
            done_after_ms: None,
            published,
            readback,
        });
        Ok(())
    }

    /// Reads the engine's events: the GPU's pass timings, and whether the frame in flight has
    /// completed (any caller's poll may be the one that sees it).
    fn pump(&mut self) -> Result<renderlab::engine::Events> {
        let events = self.engine.poll()?;
        if let Some(sample) = events.gpu.last() {
            let ms = |pass: Option<std::time::Duration>| pass.map(|d| d.as_secs_f64() * 1000.);
            self.gpu_ms = ms(Some(sample.render_span));
            // Renderlab's own split of that span; what is left over is its GI cache.
            self.passes = [
                ("Shading", Some(sample.mesh_pass)),
                ("Receivers", sample.receiver_pass),
                ("Visibility", sample.visibility_compute),
                ("Shadows", sample.shadow_generation),
                ("Reconstruction", sample.reconstruction_pass),
                ("Present", sample.present_pass),
            ]
            .map(|(name, pass)| ms(pass).map_or(("", 0.), |ms| (name, ms)));
        }
        for pending in &mut self.pending {
            if events
                .completed
                .iter()
                .any(|c| c.frame == Some(pending.frame))
            {
                pending.done = true;
                pending.done_after_ms = Some(pending.since.elapsed().as_secs_f64() * 1000.);
            }
        }
        Ok(events)
    }

    /// Takes the oldest frame [`Self::begin`] started if the GPU has finished it: `None` while it
    /// is still drawing (without waiting), or when none is in flight. A frame the engine never
    /// reports done is taken after `FRAME_WAIT`: the handoff waits for it itself.
    pub fn finish(&mut self, gpu: &mut GpuDevice) -> Result<Option<ExternalFrame>> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        self.pump()?;
        let Some(oldest) = self.pending.front() else {
            return Ok(None);
        };
        if !oldest.done && oldest.since.elapsed() <= FRAME_WAIT {
            return Ok(None);
        }
        let Some(taken) = self.pending.pop_front() else {
            return Ok(None);
        };
        let target = self
            .target
            .as_mut()
            .context("a frame is in flight without a target")?;

        // The handoff onto the presenter's device, timed: the shared ring's delivery,
        // or the CPU copy's readback and upload.
        let started = Instant::now();
        let mut id_texels = None;
        let image = if let Some(shared) = &mut target.shared {
            if !taken.published {
                // Only reached when the slot was busy and the frame before it never delivered
                // (it timed out); the output may already hold a later frame.
                shared.publish(&self.engine, taken.view)?;
            }
            settle(gpu, target.image, &mut target.fresh)?;
            shared.deliver(gpu, target.image)?;
            if shared.id_pending() {
                crate::shared::PresentedId::copy_from(&mut target.id, shared, gpu, target.size)?;
            }
            // The slot is free: the next frame waiting for it goes out now.
            if let Some(next) = self.pending.iter_mut().find(|p| !p.published) {
                shared.publish(&self.engine, next.view)?;
                next.published = true;
            }
            target.image
        } else {
            let (colour, ids) = taken
                .readback
                .context("a CPU-copy frame has its readback queued at begin")?;
            let rgba8 = colour.take(self.engine.device())?;
            id_texels = ids.map(|ids| ids.take(self.engine.device())).transpose()?;
            upload(gpu, target, &rgba8)?
        };
        let handoff_ms = started.elapsed().as_secs_f64() * 1000.;
        log_line(|| {
            format!(
                "bridge_log thread={:?} frame={} begin={:.2} done_after={} in_flight={:.2} handoff={handoff_ms:.2} still_pending={}",
                std::thread::current().id(),
                taken.frame.0,
                taken.begin_ms,
                taken.done_after_ms.map_or("none".into(), |ms| format!("{ms:.2}")),
                taken.since.elapsed().as_secs_f64() * 1000.,
                self.pending.len(),
            )
        });

        Ok(Some(ExternalFrame {
            image,
            id: target.id.as_ref().map(crate::shared::PresentedId::output),
            id_texels,
            gpu_ms: self.gpu_ms,
            passes: self.passes,
            handoff_ms,
        }))
    }

    /// The presenter-device image for `size` (recreated on a resize), with the
    /// shared ring wherever the two devices can share images.
    fn ensure_target(&mut self, gpu: &mut GpuDevice, size: [u32; 2]) -> Result<()> {
        if self.target.as_ref().is_some_and(|t| t.size != size) {
            self.release(gpu)?;
        }
        if self.target.is_some() {
            return Ok(());
        }
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let shared = if self.cpu_copy {
            None
        } else {
            match SharedHandoff::new(&self.engine, gpu, size) {
                Ok(shared) => Some(shared),
                Err(error) => {
                    // Once: the reason does not change with the size.
                    eprintln!(
                        "Ara presentation: Renderlab frames cross to the presenter by CPU copy ({error:#})"
                    );
                    self.cpu_copy = true;
                    None
                }
            }
        };
        self.target = Some(Target {
            image,
            size,
            staging: None,
            fresh: true,
            shared,
            id: None,
            ids: None,
        });
        Ok(())
    }

    /// Frame-level features this path has no mapping for yet.
    fn report_unsupported(&mut self, view: &ExternalView<'_>) {
        let frame = view.frame;
        if frame.terrain.is_some() {
            self.gaps.unsupported(
                "terrain and foliage levels of detail and foliage wind",
                "resident tiles draw at full resolution; foliage stands still at its nearest level of detail",
            );
        }
        if view.look.ev100.is_none() {
            self.gaps.unsupported(
                "Ara's automatic exposure metering",
                "the bridge meters the displayed image toward middle grey instead",
            );
        }
        if view.debug_switches {
            self.gaps.unsupported(
                "rendering debug switches and diagnostic views",
                "the frame renders lit, with every effect on",
            );
        }
        if frame.other_atmosphere_light.is_some() {
            self.gaps.unsupported(
                "a second atmosphere light (moon)",
                "only the sun lights the sky",
            );
        }
    }

    pub fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        // Frames in flight draw into the target and queue their copy into its shared slots: wait
        // for Renderlab's queue to go idle before they go with it (a resize, Play, shutdown).
        if !self.pending.is_empty() {
            self.pending.clear();
            let idle = wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(FRAME_WAIT),
            };
            if let Err(error) = self.engine.device().poll(idle) {
                eprintln!("Ara presentation: Renderlab did not go idle before its target was released ({error})");
            }
        }
        if let Some(target) = self.target.take() {
            // The import hands its slots back on this device before the image goes.
            drop(target.shared);
            if let Some(id) = target.id {
                id.destroy(gpu)?;
            }
            gpu.retire(Resource::Image(target.image))?;
            if let Some(staging) = target.staging {
                gpu.retire(Resource::Buffer(staging))?;
            }
            gpu.collect()?;
        }
        Ok(())
    }
}

/// Whether the view moved further in one frame than a continuous camera does: more
/// than 45 degrees, or more than 20 metres.
fn jumped(last: ([f32; 3], [f32; 3]), now: ([f32; 3], [f32; 3])) -> bool {
    let dot: f32 = last.1.iter().zip(now.1).map(|(a, b)| a * b).sum();
    let moved: f32 = last
        .0
        .iter()
        .zip(now.0)
        .map(|(a, b)| (a - b) * (a - b))
        .sum();
    dot < std::f32::consts::FRAC_1_SQRT_2 || moved > 20. * 20.
}

/// Brings a new target image to `CopySource`, where the shared copy expects it.
fn settle(gpu: &mut GpuDevice, image: Image, fresh: &mut bool) -> Result<()> {
    if !*fresh {
        return Ok(());
    }
    let mut commands = gpu.begin(Queue::Graphics);
    for (before, after) in [
        (Access::Undefined, Access::CopyDestination),
        (Access::CopyDestination, Access::CopySource),
    ] {
        commands.barriers([Barrier {
            resource: Resource::Image(image),
            before,
            after,
            handoff: None,
        }]);
    }
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    *fresh = false;
    Ok(())
}

/// The CPU copy: uploads display-encoded RGBA8 rows into the target and waits.
fn upload(gpu: &mut GpuDevice, target: &mut Target, rgba8: &[u8]) -> Result<Image> {
    let bytes = u64::from(target.size[0]) * u64::from(target.size[1]) * 4;
    anyhow::ensure!(
        rgba8.len() as u64 == bytes,
        "Renderlab returned {} bytes for a {:?} frame",
        rgba8.len(),
        target.size
    );
    let staging = match target.staging {
        Some(staging) => staging,
        None => *target.staging.insert(gpu.create_buffer(
            bytes,
            vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )?),
    };
    // Every earlier use of the staging buffer was resolved below.
    gpu.upload(staging, rgba8)?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::BeginPass("Renderlab.Handoff".into()));
    commands.barriers([Barrier {
        resource: Resource::Image(target.image),
        before: if target.fresh {
            Access::Undefined
        } else {
            Access::CopySource
        },
        after: Access::CopyDestination,
        handoff: None,
    }]);
    commands.push(Command::UploadImage {
        source: staging,
        destination: target.image,
    });
    commands.barriers([Barrier {
        resource: Resource::Image(target.image),
        before: Access::CopyDestination,
        after: Access::CopySource,
        handoff: None,
    }]);
    commands.push(Command::EndPass);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    target.fresh = false;
    Ok(target.image)
}
