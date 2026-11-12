//! Drives Renderlab's ara-gpu backend from extracted frames on Apple. The backend
//! renders on the presenter's own device (Metal through ara-gpu), so a frame needs no
//! second device and no copy: the returned image is the backend's displayed image,
//! left in `CopySource` on that device. The scene mapping is the one the wgpu path
//! uses (`scene`, `terrain`, `decals`, `effects`); what the backend does not draw yet
//! it reports, and the bridge passes each report on once (`ARA_RENDERLAB_UNSUPPORTED`).
//! A frame splits as on the wgpu path: `begin` submits it and returns, `finish` takes it
//! once the GPU has finished it, without waiting; `render` is the two with a wait between.
use crate::{
    ActivePath, Options,
    decals::Decals,
    effects::Effects,
    gaps::Gaps,
    lighting::{self, Metering},
    rl::Engine,
    scene::{MAX_GROUPS, Scene},
    sky::Skies,
    terrain::Terrain,
};
use anyhow::{Context, Result, anyhow};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Queue, Resource, TimelinePoint,
    commands::Command,
};
use ara_runtime_render::{EXTERNAL_PASSES, ExternalFrame, ExternalView};
use renderlab::{
    IdLayer, ViewId,
    backend_ara::{cache::ShaderCache, histogram::HistogramDesc},
    engine::{Light, Outputs, Settings, Sun, ViewDesc, ViewLook},
};

/// Lighting last applied to the engine, so unchanged frames set nothing.
#[derive(Default)]
struct Applied {
    sun: Option<Sun>,
    lights: Vec<Light>,
    /// The colour grade last asked for (`grade::key`).
    grade: Option<u64>,
}

/// A frame submitted and not yet taken: its displayed image, its packed ids with their
/// readback, the point queued behind them, and the newest finished frame's GPU times.
struct Pending {
    image: Image,
    ids: Option<(Image, Buffer)>,
    point: TimelinePoint,
    gpu_ms: Option<f64>,
    passes: [(&'static str, f64); EXTERNAL_PASSES],
}

pub(crate) struct Bridge {
    engine: Engine,
    scene: Scene,
    terrain: Terrain,
    decals: Decals,
    effects: Effects,
    gaps: Gaps,
    applied: Applied,
    skies: Skies,
    metering: Metering,
    /// The meter's histograms in flight: each request with the meter and exposure of the
    /// frame it binned.
    metered: Vec<(u64, f32, f32)>,
    /// The frame [`Bridge::begin`] submitted, until [`Bridge::finish`] takes it.
    pending: Option<Pending>,
    max_reflection_roughness: Option<f32>,
    /// The one view the bridge renders (the presenter's), created on the first frame.
    view: Option<ViewId>,
    /// What the view is set to: its size and look (its colour table is baked,
    /// `Applied::grade`).
    desc: ViewDesc,
    /// The look last applied to the view without its table, so unchanged frames set
    /// nothing.
    last_look: Option<ViewLook>,
    cut: bool,
    /// The last frame's eye and view direction, to notice a view that jumps.
    last_view: Option<([f32; 3], [f32; 3])>,
}

impl Bridge {
    pub fn new(options: &Options) -> Result<Self> {
        if options.path == crate::PathChoice::NonRt {
            anyhow::bail!(
                "ARA_RENDERLAB_UNAVAILABLE: Renderlab's Metal backend has the hardware-ray path only"
            );
        }
        // The backend's cooked shaders and their reflection, kept apart from Ara's.
        let root = options
            .pipeline_cache
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("ara-renderlab-metal"));
        let cache = ShaderCache::new(root.join("renderlab-metal")).map_err(|error| {
            anyhow!(
                "ARA_RENDERLAB_UNAVAILABLE: Renderlab's Metal backend could not start: {error:#}"
            )
        })?;
        let mut engine = Engine::new(cache, Settings::default()).map_err(|error| {
            anyhow!("ARA_RENDERLAB_UNAVAILABLE: Renderlab's Metal backend could not start: {error}")
        })?;
        // The presenter copies the frame from the device: nothing comes back to the CPU,
        // and every frame reports its passes' GPU times.
        engine.set_readback(false);
        eprintln!("Ara presentation: Renderlab (HardwareRt) on the presenter's Metal device");
        Ok(Self {
            engine,
            scene: Scene::default(),
            terrain: Terrain::default(),
            decals: Decals::default(),
            effects: Effects::default(),
            gaps: Gaps::default(),
            applied: Applied::default(),
            skies: Skies::new(options.constant_sky),
            metering: Metering::new(),
            metered: Vec::new(),
            pending: None,
            max_reflection_roughness: options.max_reflection_roughness,
            view: None,
            // The Scene host's ids (`Options::scene_ids`): the view writes them beside
            // each frame.
            desc: ViewDesc {
                outputs: Outputs {
                    ids: options.scene_ids,
                },
                ..ViewDesc::new(1, 1)
            },
            last_look: None,
            cut: true,
            last_view: None,
        })
    }

    pub fn path(&self) -> ActivePath {
        ActivePath::HardwareRt
    }

    pub fn cut(&mut self) {
        self.cut = true;
    }

    /// No separate answer: the backend's front ids are its opaque ones (it draws no glass
    /// ids yet), so a click reads the delivered texels (`ExternalFrame::id_texels`).
    pub fn pick_id(&mut self, _pixel: [u32; 2]) -> Result<Option<u32>> {
        Ok(None)
    }

    /// Starts drawing `view` and returns without waiting for the GPU: the scene is synced and
    /// the frame, its ids' pack and copy and the meter's histogram are submitted.
    /// [`Self::finish`] takes the result. One frame is in flight at a time: it is an error to
    /// begin another before it is taken.
    pub fn begin(&mut self, gpu: &mut GpuDevice, view: &ExternalView<'_>) -> Result<()> {
        anyhow::ensure!(
            self.pending.is_none(),
            "a frame is in flight: finish it before beginning another"
        );
        let frame = view.frame;
        self.report_unsupported(view);
        // Terrain tiles take at most half the mesh-material groups; entity draws and
        // foliage share the rest.
        let (tiles, _) =
            self.terrain
                .sync(&mut self.engine, view, MAX_GROUPS / 2, None, &mut self.gaps)?;
        let (foliage, foliage_key) = self.terrain.foliage();
        self.scene.sync(
            &mut self.engine,
            frame,
            foliage,
            foliage_key,
            tiles,
            &mut self.gaps,
        )?;
        // Terrain tiles follow the frame's draws in the editor's id numbering.
        self.terrain
            .tag(&mut self.engine, frame.draws.len() as u32 + 1)?;
        self.decals.sync(&mut self.engine, frame, &mut self.gaps)?;
        self.effects.sync(&mut self.engine, frame, &mut self.gaps)?;

        // The meter's readings of earlier frames, taken before this frame's exposure is
        // chosen, as the wgpu bridge takes its own: usually the frame before last. Readings
        // that finish while automatic exposure is off are dropped.
        let automatic = view.look.ev100.is_none();
        for reading in self.engine.histograms(gpu)? {
            let at = self.metered.iter().position(|m| m.0 == reading.request);
            if let Some((_, meter, stops)) = at.map(|at| self.metered.remove(at))
                && automatic
            {
                self.metering
                    .observe(view.look, meter, stops, &reading.bins);
            }
        }
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
        // The view: created on the first frame; a new size or look sets it (a size
        // restarts its frame sequence, as a resize did before views).
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
            self.metering.snap();
            // Readings in flight describe the old view.
            self.metered.clear();
        }
        self.engine
            .render_view(gpu, view_id, &lighting::camera(frame)?)?;
        for refusal in self.engine.refusals() {
            self.gaps.unsupported(
                refusal.what,
                "Renderlab's Metal backend leaves it out of the frame",
            );
        }
        let image = self
            .engine
            .view_output(view_id)
            .context("Renderlab's Metal backend produced no displayed image")?
            .image;
        // The meter: on the frames it asks for (every frame while it snaps, then every
        // 16th), the displayed image is binned on the GPU as the wgpu path's histogram pass
        // bins it, without waiting; the reading arrives on a later frame. One refused (four
        // in flight) is asked for again next frame.
        if let Some(meter) = metered
            && self.metering.due()
        {
            let desc = HistogramDesc {
                bins: crate::meter::BINS as u32,
                log_min: crate::meter::LOG_DARKEST,
                stride: crate::meter::STRIDE,
            };
            match self.engine.view_histogram(gpu, view_id, desc) {
                Ok(request) => self.metered.push((request, meter, stops)),
                Err(_) => self.metering.retry(),
            }
        }
        let (gpu_ms, passes) = self
            .engine
            .images(view_id)
            .map(|images| {
                (
                    Some(images.times.gpu.as_secs_f64() * 1000.),
                    groups(&images.times.passes),
                )
            })
            .unwrap_or((None, [("", 0.); EXTERNAL_PASSES]));
        // The view's opaque ids packed into RGBA8 on this device, and a copy of the same
        // texels for clicks on the frames of a host that asked; otherwise an empty submission.
        // Either is queued behind the frame: its point says when the frame is done.
        let (ids, point) = if self.desc.outputs.ids {
            let packed = self.engine.view_ids_packed(gpu, view_id, IdLayer::Opaque)?;
            let (packed, size) = (packed.image, packed.size);
            let (readback, point) = copy_back(gpu, "Renderlab.Ids", packed, size)?;
            (Some((packed, readback)), point)
        } else {
            (None, gpu.submit(gpu.begin(Queue::Graphics).finish(), &[])?)
        };
        self.pending = Some(Pending {
            image,
            ids,
            point,
            gpu_ms,
            passes,
        });
        Ok(())
    }

    /// Whether a frame was begun and not yet taken by [`Self::finish`].
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Takes the frame [`Self::begin`] started once the GPU has finished it, with its ids'
    /// texels; `None` while it draws or when none is in flight. Never waits for the GPU.
    pub fn finish(&mut self, gpu: &mut GpuDevice) -> Result<Option<ExternalFrame>> {
        let Some(pending) = &self.pending else {
            return Ok(None);
        };
        if gpu.completed()?[pending.point.queue.index()] < pending.point.value {
            return Ok(None);
        }
        let pending = self.pending.take().context("checked above")?;
        let (id, id_texels) = match pending.ids {
            Some((packed, readback)) => {
                let texels = gpu.read_completed(readback, pending.point);
                gpu.retire(Resource::Buffer(readback))?;
                (Some(packed), Some(texels?))
            }
            None => (None, None),
        };
        Ok(Some(ExternalFrame {
            image: pending.image,
            id,
            id_texels,
            gpu_ms: pending.gpu_ms,
            passes: pending.passes,
            // The frame was drawn on the presenter's device: nothing crossed.
            handoff_ms: 0.,
        }))
    }

    /// Draws `view` and returns its completed frame: [`Self::begin`], a wait for it, then
    /// [`Self::finish`].
    pub fn render(
        &mut self,
        gpu: &mut GpuDevice,
        view: &ExternalView<'_>,
    ) -> Result<ExternalFrame> {
        self.begin(gpu, view)?;
        let point = self.pending.as_ref().context("begun above")?.point;
        gpu.wait(point)?;
        self.finish(gpu)?
            .context("the frame was waited for and is not done")
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
        self.engine.release(gpu)?;
        gpu.collect()?;
        Ok(())
    }
}

/// The backend's labelled passes in the six groups `ExternalFrame::passes` holds.
fn groups(passes: &[(String, f64)]) -> [(&'static str, f64); EXTERNAL_PASSES] {
    const NAMES: [&str; EXTERNAL_PASSES] = [
        "Shading",
        "Receivers",
        "IndirectCache",
        "Reconstruction",
        "Post",
        "Other",
    ];
    let mut sums = [0.; EXTERNAL_PASSES];
    for (label, ms) in passes {
        let pass = label.trim_start_matches("ara::");
        let group = if pass.starts_with("PrimaryShade") || pass.starts_with("ProgramBins") {
            0
        } else if pass.starts_with("PrimaryInteractive") || pass.starts_with("PrimaryTrace") {
            1
        } else if pass.starts_with("GiCache") {
            2
        } else if pass.starts_with("TemporalResolve") {
            3
        } else if [
            "PostPrimarySurface",
            "TileMax",
            "MotionBlur",
            "Downsample",
            "Upsample",
            "Tone",
            "DisplayTransfer",
            "DepthOfField",
        ]
        .iter()
        .any(|p| pass.starts_with(p))
        {
            4
        } else {
            5
        };
        sums[group] += ms;
    }
    std::array::from_fn(|i| (NAMES[i], sums[i]))
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

/// Copies an RGBA8 image's rows (the packed ids) into a new host-readable buffer on the
/// presenter's device, in a pass labelled `label`, and submits without waiting: the rows are
/// readable once the returned point completes. `image` is in `CopySource`, and stays there.
fn copy_back(
    gpu: &mut GpuDevice,
    label: &str,
    image: Image,
    size: [u32; 2],
) -> Result<(Buffer, TimelinePoint)> {
    let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
    let buffer = gpu.create_readback_buffer(bytes)?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::BeginPass(label.into()));
    commands.barriers([Barrier {
        resource: Resource::Buffer(buffer),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    commands.push(Command::ReadImage {
        source: image,
        destination: buffer,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(buffer),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    commands.push(Command::EndPass);
    let point = gpu.submit(commands.finish(), &[])?;
    Ok((buffer, point))
}
