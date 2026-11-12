//! Drives Renderlab's `Engine` from extracted frames and hands each image to the
//! presenter's device. Renderlab renders on its own device: where the two devices
//! can share images the frame crosses through a shared ring (`shared`), otherwise by
//! a CPU copy (a readback there, an upload here). Either crossing is timed.
use crate::{
    ActivePath, Options, PathChoice,
    decals::Decals,
    effects::Effects,
    gaps::Gaps,
    lent::Lent,
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

/// What the frame worker works on: the engine and everything its frame jobs touch. It is
/// away on the worker while a frame is recorded and submitted (`Bridge::begin`) and home
/// again when the studio thread next needs the engine (`Bridge::finish`, a click, the next
/// frame's sync).
struct Rig {
    /// Declared before `engine`: the shared ring lives on the engine's device and
    /// must drop first.
    target: Option<Target>,
    engine: Engine,
    /// The GPU histogram passes in flight for the meter.
    readbacks: Readbacks,
    /// What the meter's passes report on the worker (the bridge's `gaps` stays on the
    /// studio thread).
    gaps: Gaps,
    /// What the last job did, until `Bridge::reap` takes it.
    drawn: Option<Result<Drawn>>,
}

/// One frame's job for the worker.
struct Draw {
    view: ViewId,
    camera: renderlab::engine::Camera,
    size: [u32; 2],
    /// Ids are written beside the frame (`Options::scene_ids`).
    scene_ids: bool,
    /// The meter's reading and exposure to queue a histogram pass for, when it is due.
    meter: Option<(f32, f32)>,
}

/// What a frame job produced.
struct Drawn {
    frame: renderlab::engine::FrameId,
    /// Whether the meter's pass was queued, when one was asked for.
    meter_queued: Option<bool>,
}

impl Rig {
    /// Records and submits `draw`'s frame, then queues the work that follows it: the ids, the
    /// copy into the shared slot and the meter's pass. Runs on the frame worker.
    fn draw(&mut self, draw: Draw) -> Result<Drawn> {
        let frame = self.engine.render_view(draw.view, &draw.camera)?.frame;
        let target = self
            .target
            .as_mut()
            .context("the frame was begun without a target")?;
        if draw.scene_ids {
            if target.ids.as_ref().is_none_or(|t| t.size() != draw.size) {
                target.ids = Some(crate::id_target::IdTarget::new(
                    self.engine.device(),
                    draw.size,
                ));
            }
            if let Some(ids) = &target.ids {
                ids.write(&mut self.engine, draw.view)?;
            }
        }
        // Shared: the copy into the shared slot queues behind the frame right away.
        if let Some(shared) = &mut target.shared {
            match &target.ids {
                Some(ids) => shared.set_id(ids.texture().clone()),
                None if crate::shared::scene_ids_pattern() => {
                    shared.stage_pattern(&self.engine)?;
                }
                None => {}
            }
            shared.publish(&self.engine, draw.view)?;
        }
        // The meter: it does not wait; a refused pass is asked for again next frame.
        let meter_queued = draw.meter.map(|(meter, stops)| {
            self.engine.view_output(draw.view).is_some_and(|output| {
                self.readbacks.submit(
                    self.engine.device(),
                    self.engine.queue(),
                    output,
                    meter,
                    stops,
                    &mut self.gaps,
                )
            })
        });
        Ok(Drawn {
            frame,
            meter_queued,
        })
    }
}

/// A frame begun and not yet taken.
struct Pending {
    /// The engine's id for it, known once the worker has submitted it.
    frame: Option<renderlab::engine::FrameId>,
    view: ViewId,
    since: Instant,
    /// The engine reported it complete.
    done: bool,
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
    /// The engine and its frame state, on the frame worker while a frame is submitted.
    rig: Lent<Rig>,
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
    /// The frame begun and not yet taken (`Bridge::begin`, `Bridge::finish`).
    pending: Option<Pending>,
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
        let rig = Lent::new(
            "renderlab-frame",
            Rig {
                target: None,
                engine,
                readbacks,
                gaps: Gaps::default(),
                drawn: None,
            },
        )
        .context("Renderlab could not start its frame worker")?;
        Ok(Self {
            rig,
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
            max_reflection_roughness: options.max_reflection_roughness,
            skies: Skies::new(options.constant_sky),
            cpu_copy: options.cpu_copy_handoff,
            scene_ids: options.scene_ids,
            gpu_ms: None,
            passes: [("", 0.); EXTERNAL_PASSES],
            cut: true,
            pending: None,
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
        // The ids are of the frame the worker may still be submitting.
        self.rig.wait();
        let Ok(request) = self
            .rig
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

    /// Draws `view` and waits for it: [`Self::begin`], then [`Self::finish`] until it is done.
    pub fn render(
        &mut self,
        gpu: &mut GpuDevice,
        view: &ExternalView<'_>,
    ) -> Result<ExternalFrame> {
        self.begin(gpu, view)?;
        loop {
            if let Some(frame) = self.finish(gpu)? {
                return Ok(frame);
            }
            std::thread::yield_now();
        }
    }

    /// Whether a frame was begun and not yet taken by [`Self::finish`].
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Starts drawing `view` and returns without waiting for the GPU: the scene is synced and the
    /// frame, its ids and the meter's pass are submitted. [`Self::finish`] takes the result. Only
    /// one frame is in flight at a time: it is an error to begin another before it is taken.
    pub fn begin(&mut self, gpu: &mut GpuDevice, view: &ExternalView<'_>) -> Result<()> {
        anyhow::ensure!(
            self.pending.is_none(),
            "a frame is in flight: finish it before beginning another"
        );
        // The sync below needs the engine on this thread.
        self.rig.wait();
        let frame = view.frame;
        // The meter's readings of earlier frames, the previous one's included: the bridge
        // waited for that frame and its histogram pass is queued right behind it, so the
        // reading is done by now. They are taken before this frame's exposure is chosen, so
        // a cut's reading is applied one frame later, as it was when the meter read the
        // frame back. Readings that finish while automatic exposure is off are dropped, not
        // kept for when it comes back on.
        let automatic = view.look.ev100.is_none();
        let rig = self.rig.wait();
        for reading in rig.readbacks.poll(rig.engine.device(), &mut rig.gaps) {
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
                &mut self.rig.engine,
                view,
                TERRAIN_GROUPS,
                nonrt.then_some(known),
                &mut self.gaps,
            )?;
            let (foliage, foliage_key) = self.terrain.foliage();
            self.scene.sync(
                &mut self.rig.engine,
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
            .tag(&mut self.rig.engine, frame.draws.len() as u32 + 1)?;
        self.decals
            .sync(&mut self.rig.engine, frame, &mut self.gaps)?;
        self.effects
            .sync(&mut self.rig.engine, frame, &mut self.gaps)?;

        let (stops, metered) = lighting::exposure_stops(view, &mut self.metering);
        let (settings, look) =
            lighting::settings(view, stops, self.max_reflection_roughness, &mut self.gaps);
        self.rig.engine.set_settings(settings)?;
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
                Some(id) => self.rig.engine.set_view(id, &self.desc)?,
                None => self.view = Some(self.rig.engine.create_view(&self.desc)?),
            }
            self.last_look = Some(look);
        }
        let view_id = self.view.context("the view was created above")?;
        self.skies
            .apply(&mut self.rig.engine, view, &mut self.gaps, self.cut)?;
        let sun = lighting::sun(frame);
        if self.applied.sun != sun {
            self.rig.engine.set_sun(sun)?;
            self.applied.sun = sun;
        }
        let lights = lighting::lights(
            frame.lights.iter().chain(self.effects.lights()),
            &mut self.gaps,
        );
        if self.applied.lights != lights {
            self.rig.engine.set_lights(&lights)?;
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
            self.rig.engine.cut_view(view_id);
            // As Ara resets its exposure history: the meter snaps to its target.
            self.metering.snap();
            // Readings in flight describe the old view.
            self.rig.readbacks.discard();
        }
        self.ensure_target(gpu, view.size)?;
        let camera = lighting::camera(frame)?;
        // The meter's pass: asked for every frame while it snaps, then every 16th.
        let meter = metered.filter(|_| self.metering.due());
        let draw = Draw {
            view: view_id,
            camera,
            size: view.size,
            scene_ids: self.scene_ids,
            meter: meter.map(|meter| (meter, stops)),
        };
        // The frame is recorded and submitted on the worker, with the ids, the copy into the
        // shared slot and the meter's pass behind it; this thread is free until the GPU is
        // done (`Self::finish`).
        self.rig.send(move |rig| rig.drawn = Some(rig.draw(draw)));
        self.pending = Some(Pending {
            frame: None,
            view: view_id,
            since: Instant::now(),
            done: false,
        });
        Ok(())
    }

    /// Takes the result of the frame job once the worker has handed the engine back: the
    /// frame's id, or why it could not be drawn.
    fn reap(&mut self) -> Result<()> {
        let Some(drawn) = self.rig.wait().drawn.take() else {
            return Ok(());
        };
        match drawn {
            Ok(drawn) => {
                if let Some(pending) = &mut self.pending {
                    pending.frame = Some(drawn.frame);
                }
                if drawn.meter_queued == Some(false) {
                    self.metering.retry();
                }
                Ok(())
            }
            Err(error) => {
                self.pending = None;
                Err(error)
            }
        }
    }

    /// Reads the engine's events: the GPU's pass timings, and whether the frame in flight has
    /// completed (any caller's poll may be the one that sees it).
    fn pump(&mut self) -> Result<renderlab::engine::Events> {
        self.reap()?;
        let events = self.rig.engine.poll()?;
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
        if let Some(pending) = &mut self.pending
            && events
                .completed
                .iter()
                .any(|c| c.frame.is_some() && c.frame == pending.frame)
        {
            pending.done = true;
        }
        Ok(events)
    }

    /// Takes the frame [`Self::begin`] started if the GPU has finished it: `None` while it is
    /// still drawing (without waiting), or when none is in flight. A frame the engine never
    /// reports done is taken after `FRAME_WAIT`: the handoff waits for it itself.
    pub fn finish(&mut self, gpu: &mut GpuDevice) -> Result<Option<ExternalFrame>> {
        if self.pending.is_none() {
            return Ok(None);
        }
        // The worker is still recording or submitting it.
        if !self.rig.try_home() {
            return Ok(None);
        }
        self.pump()?;
        let Some(pending) = &self.pending else {
            return Ok(None);
        };
        if !pending.done && pending.since.elapsed() <= FRAME_WAIT {
            return Ok(None);
        }
        let view_id = pending.view;
        self.pending = None;
        let rig = &mut *self.rig;
        let target = rig
            .target
            .as_mut()
            .context("a frame is in flight without a target")?;

        // The handoff onto the presenter's device, timed: the shared ring's delivery,
        // or the CPU copy's readback and upload.
        let started = Instant::now();
        let mut id_texels = None;
        let image = if let Some(shared) = &mut target.shared {
            settle(gpu, target.image, &mut target.fresh)?;
            shared.deliver(gpu, target.image)?;
            if shared.id_pending() {
                crate::shared::PresentedId::copy_from(&mut target.id, shared, gpu, target.size)?;
            }
            target.image
        } else {
            let read = rig.engine.capture_view(view_id)?;
            id_texels = target
                .ids
                .as_ref()
                .map(|ids| ids.read(&rig.engine))
                .transpose()?;
            upload(gpu, target, &read.rgba8)?
        };
        let handoff_ms = started.elapsed().as_secs_f64() * 1000.;

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
        if self.rig.target.as_ref().is_some_and(|t| t.size != size) {
            self.release(gpu)?;
        }
        if self.rig.target.is_some() {
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
            match SharedHandoff::new(&self.rig.engine, gpu, size) {
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
        self.rig.target = Some(Target {
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
        // A frame in flight draws into the target: it goes with it.
        self.pending = None;
        // The worker may still be drawing into the target: the engine comes home first.
        let rig = self.rig.wait();
        rig.drawn = None;
        if let Some(target) = rig.target.take() {
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
