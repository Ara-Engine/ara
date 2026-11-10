//! Drives Renderlab's `Engine` from extracted frames and hands each image to the
//! presenter's device. Renderlab renders on its own device: where the two devices
//! can share images the frame crosses through a shared ring (`shared`), otherwise by
//! a CPU copy (a readback there, an upload here). Either crossing is timed.
use crate::{
    ActivePath, Options, PathChoice,
    decals::Decals,
    environment,
    gaps::Gaps,
    lighting::{self, Metering, SkySource},
    scene::{MAX_GROUPS, Scene},
    shared::SharedHandoff,
    terrain::Terrain,
};
use anyhow::{Context, Result, anyhow};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk,
};
use ara_runtime_render::{EXTERNAL_PASSES, ExternalFrame, ExternalView};
use renderlab::engine::{
    AtmosphereMedium, Background, Capabilities, Engine, EngineDescriptor, EnvironmentId,
    ExperimentalRayQuery, Light, Limits, RenderPath, Sky, Sun, headless_adapter,
};
use std::time::{Duration, Instant};

/// Longest a present waits for its frame before reading it back regardless.
const FRAME_WAIT: Duration = Duration::from_secs(5);

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
}

/// Lighting last applied to the engine, so unchanged frames set nothing.
#[derive(Default)]
struct Applied {
    sun: Option<Sun>,
    lights: Vec<Light>,
    sky: Option<Sky>,
    /// The prepared sky map in use and the content key it was prepared from.
    sky_map: Option<(u64, EnvironmentId)>,
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
    gaps: Gaps,
    applied: Applied,
    metering: Metering,
    max_reflection_roughness: Option<f32>,
    /// A sky image lights the scene as a constant of its mean radiance (`Options`).
    constant_sky: bool,
    /// The frames cross by CPU copy: asked for, or the shared route is unavailable.
    cpu_copy: bool,
    /// Frames until the shared route next reads a frame back for the meter.
    meter_wait: u32,
    gpu_ms: Option<f64>,
    /// Renderlab's own passes for the frame `gpu_ms` measured (`ExternalFrame::passes`).
    passes: [(&'static str, f64); EXTERNAL_PASSES],
    cut: bool,
    /// The last frame's eye and view direction, to notice a view that jumps.
    last_view: Option<([f32; 3], [f32; 3])>,
    /// The sky being prepared off the frame loop, if any.
    sky_job: Option<SkyJob>,
    /// Replaced sky maps waiting for the next sky change to be removed.
    stale_maps: Vec<EnvironmentId>,
}

/// One sky image being prepared for `Sky::Map` on a worker thread.
struct SkyJob {
    /// Content key of the image (`environment::key`).
    key: u64,
    worker: std::thread::JoinHandle<Result<renderlab::engine::PreparedEnvironment>>,
}

impl Bridge {
    pub fn new(options: &Options) -> Result<Self> {
        let adapter = headless_adapter().map_err(|error| {
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
        Ok(Self {
            target: None,
            engine,
            path,
            scene: Scene::default(),
            terrain: Terrain::default(),
            decals: Decals::default(),
            gaps: Gaps::default(),
            applied: Applied::default(),
            metering: Metering::new(),
            max_reflection_roughness: options.max_reflection_roughness,
            constant_sky: options.constant_sky,
            cpu_copy: options.cpu_copy_handoff,
            meter_wait: 0,
            gpu_ms: None,
            passes: [("", 0.); EXTERNAL_PASSES],
            cut: true,
            last_view: None,
            sky_job: None,
            stale_maps: Vec::new(),
        })
    }

    pub fn path(&self) -> ActivePath {
        self.path
    }

    pub fn cut(&mut self) {
        self.cut = true;
    }

    pub fn render(
        &mut self,
        gpu: &mut GpuDevice,
        view: &ExternalView<'_>,
    ) -> Result<ExternalFrame> {
        let frame = view.frame;
        self.report_unsupported(view);
        self.engine.resize(view.size[0], view.size[1])?;
        // Terrain tiles take at most half the mesh-material groups; entity draws and
        // foliage share the rest.
        let tiles = self
            .terrain
            .sync(&mut self.engine, view, MAX_GROUPS / 2, &mut self.gaps)?;
        let (foliage, foliage_key) = self.terrain.foliage();
        self.scene.sync(
            &mut self.engine,
            frame,
            foliage,
            foliage_key,
            tiles,
            &mut self.gaps,
        )?;
        self.decals.sync(&mut self.engine, frame, &mut self.gaps)?;

        let (stops, metered) = lighting::exposure_stops(view, &self.metering);
        self.engine.set_settings(lighting::settings(
            view,
            stops,
            self.max_reflection_roughness,
            &mut self.gaps,
        ))?;
        self.apply_sky(view)?;
        let sun = lighting::sun(frame);
        if self.applied.sun != sun {
            self.engine.set_sun(sun)?;
            self.applied.sun = sun;
        }
        let lights = lighting::lights(&frame.lights, &mut self.gaps);
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
            self.engine.cut();
        }
        self.ensure_target(gpu, view.size)?;
        let rendered = self.engine.render(&lighting::camera(frame)?)?.frame;
        let target = self.target.as_mut().context("target ensured above")?;
        // Shared: the copy into the shared slot queues behind the frame right away.
        if let Some(shared) = &mut target.shared {
            shared.publish(&self.engine)?;
        }

        // Wait for the frame on Renderlab's device first, so the handoff below times
        // the crossing alone. Bounded: a completion the engine dropped only costs
        // the split (the handoff waits for the frame itself).
        let waited = Instant::now();
        loop {
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
            let done = events.completed.iter().any(|c| c.frame == Some(rendered));
            if done || waited.elapsed() > FRAME_WAIT {
                break;
            }
            std::thread::yield_now();
        }

        // The handoff onto the presenter's device, timed: the shared ring's delivery,
        // or the CPU copy's readback and upload.
        let started = Instant::now();
        let image = if let Some(shared) = &mut target.shared {
            settle(gpu, target.image, &mut target.fresh)?;
            shared.deliver(gpu, target.image)?;
            target.image
        } else {
            let read = self.engine.capture()?;
            if metered {
                self.metering.observe(stops, &read.rgba8);
            }
            upload(gpu, target, &read.rgba8)?
        };
        let handoff_ms = started.elapsed().as_secs_f64() * 1000.;

        // The shared route reads a frame back only for the meter: every frame while
        // the meter is moving, then every 16th.
        if metered && target.shared.is_some() {
            if self.meter_wait == 0 {
                let read = self.engine.capture()?;
                let moved = self.metering.observe(stops, &read.rgba8);
                self.meter_wait = if moved { 0 } else { 15 };
            } else {
                self.meter_wait -= 1;
            }
        }
        Ok(ExternalFrame {
            image,
            gpu_ms: self.gpu_ms,
            passes: self.passes,
            handoff_ms,
        })
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
        if !frame.effects.is_empty() {
            self.gaps
                .unsupported("VFX graph effects", "particles are not drawn");
        }
        if !frame.look.volumes.is_empty() {
            self.gaps.unsupported(
                "post-process volume grading",
                "only exposure, bloom and motion blur follow the blended look",
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

    /// Sets the frame's sky when it changed. An environment map or gradient sky is
    /// prepared once per content (`environment`) on a worker thread, a few hundred
    /// milliseconds of filtering: the frame loop keeps the sky it has (a constant of
    /// the image's mean before the first one) until the new one is ready. A capture or
    /// a cut waits for it, so stills never show a sky in transition.
    fn apply_sky(&mut self, view: &ExternalView<'_>) -> Result<()> {
        let mut replaced = None;
        let sky = match lighting::sky(view, &mut self.gaps) {
            SkySource::Atmosphere => Sky::Atmosphere(AtmosphereMedium::default()),
            SkySource::Map { size, pixels } if self.constant_sky => {
                Sky::Uniform(environment::mean_radiance(size, &pixels))
            }
            SkySource::Map { size, pixels } => {
                let key = environment::key(size, &pixels);
                loop {
                    // Take over a finished preparation (any, when waiting).
                    if let Some(job) = self
                        .sky_job
                        .take_if(|job| self.cut || job.worker.is_finished())
                    {
                        let prepared = job
                            .worker
                            .join()
                            .map_err(|_| anyhow!("the sky preparation thread panicked"))??;
                        let map = self.engine.create_environment(&prepared)?;
                        let previous = self.applied.sky_map.replace((job.key, map));
                        // Two maps can finish between sky changes: all but the last go.
                        if let Some(stale) = replaced.replace(previous.map(|(_, map)| map)) {
                            self.stale_maps.extend(stale);
                        }
                    }
                    let current = self.applied.sky_map.map(|(applied, _)| applied);
                    if current == Some(key) || self.sky_job.is_some() {
                        break;
                    }
                    let (owned, wait) = (pixels.to_vec(), self.cut);
                    let worker = std::thread::Builder::new()
                        .name("renderlab-sky".into())
                        .spawn(move || environment::prepare(size, &owned))
                        .context("sky preparation thread")?;
                    self.sky_job = Some(SkyJob { key, worker });
                    if !wait {
                        break;
                    }
                }
                match self.applied.sky_map {
                    Some((_, map)) => Sky::Map {
                        map,
                        rotation_y: 0.,
                        intensity: 1.,
                        background: Background::Sky,
                    },
                    None => Sky::Uniform(environment::mean_radiance(size, &pixels)),
                }
            }
        };
        if self.applied.sky != Some(sky) {
            self.engine.set_sky(sky)?;
            self.applied.sky = Some(sky);
        }
        // Renderlab keeps the selected map alive until the sky changes: old ones go
        // only after the new sky is set.
        for map in replaced
            .flatten()
            .into_iter()
            .chain(self.stale_maps.drain(..))
        {
            self.engine.remove_environment(map)?;
        }
        Ok(())
    }

    pub fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        if let Some(target) = self.target.take() {
            // The import hands its slots back on this device before the image goes.
            drop(target.shared);
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
