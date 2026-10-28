//! Reflection capture bakes (`look::captures`). Each face renders through a square
//! camera of its own (a secondary view's state: targets, look, cascades and native
//! culling): deferred before the temporal resolve, unjittered, sharing the probe GI
//! and reading no captures. Compute passes then project the face into the atlas's
//! scratch column, and a capture's last face reduces and prefilters it into its
//! own column. Only primary frames bake; every camera reads the result.
use super::*;
use crate::graph::Graph;
use crate::look::captures::{
    BakeStatus, CaptureImage, CaptureSpec, LEVELS, Lighting, Settled, Work, level_y,
};
use ara_gpu::{Barrier, TimelinePoint};
use ara_gpu::{
    RootConstants,
    vulkan::{Command, Recorder},
};

/// Cube face view directions (look-captures.slang `captureFaceMain` order).
const AXES: [Vec3; 6] = [
    Vec3::X,
    Vec3::NEG_X,
    Vec3::Y,
    Vec3::NEG_Y,
    Vec3::Z,
    Vec3::NEG_Z,
];

impl Renderer {
    /// Sets the reflection captures to bake: authored components or
    /// `look::captures::auto_place`, in world space. Unchanged specs keep their
    /// bakes; frames read captures while their quality has `reflection_captures`
    /// (the High/Epic PSOs shade them).
    pub fn set_reflection_captures(&mut self, captures: &[CaptureSpec]) -> Result<()> {
        self.views.captures.set_specs(captures)
    }

    /// Bakes due reflection captures before a primary frame: one step (a face or a
    /// prefilter) per live frame, every due step when `sync` (captures, deterministic
    /// frames). A failed
    /// bake turns captures off (reported once) rather than failing the frame.
    pub(in crate::renderer) fn bake_reflection_captures(&mut self, sync: bool) {
        if self.secondary_active {
            return;
        }
        if let Err(error) = self.bake_captures(sync) {
            eprintln!("ara-render: reflection captures are off after a failed bake: {error:#}");
            self.views.captures.fail();
            self.views.captures.baking = false;
        }
    }

    fn bake_captures(&mut self, sync: bool) -> Result<()> {
        let quality = self.look.quality.reflection_captures;
        if quality.is_none() {
            return Ok(());
        }
        // Bakes wait for probe GI to land, converge and fade in (they are lit by it).
        let gi = match &self.look.gi {
            None => Some(0),
            Some(gi)
                if gi.fresh
                    || !gi.landed()
                    || gi.arriving()
                    || gi.deferred_arriving()
                    || !self.look.probe_writes.is_empty() =>
            {
                None
            }
            // A pool's finer lattices landing later relight the bakes too.
            Some(gi) => {
                Some(gi.probes * 2 + u32::from(gi.deferred_received >= gi.deferred as usize))
            }
        };
        let (d, l) = (self.content.sun_direction, self.content.sun_lux);
        let lighting = Lighting {
            content: self.content.base_hash.clone(),
            sun: [d.x, d.y, d.z, l.x, l.y, l.z],
            look: (self.look.look.sky, self.look.look.fog),
            lights: light_hash(&self.content.lights),
            gi: gi.unwrap_or(0),
        };
        let settled = Settled {
            gi: gi.is_some(),
            textures: self
                .native_texture_stats()
                .is_none_or(|stats| stats.pending_textures == 0),
        };
        self.poll_capture_readbacks(sync)?;
        let work = self
            .views
            .captures
            .plan(&mut self.gpu, quality, lighting, settled, sync)?;
        // A restore rewrites the atlas (scratch column included) and restarts bakes.
        if self.views.captures.take_restore() {
            return self.restore_captures();
        }
        for item in work {
            match item {
                Work::Face {
                    face,
                    position,
                    resolution,
                    ..
                } => self.bake_face(face, position, resolution)?,
                Work::Finish {
                    capture,
                    resolution,
                } => {
                    self.prefilter_capture(capture, resolution)?;
                    self.views.captures.finished(capture);
                    if self.views.captures.is_explicit(capture) {
                        self.read_capture(capture)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Requests a bake of authored probe `key` (an explicit capture); false when the
    /// renderer holds no record of it (unknown, or past the preset's capture count).
    /// The bake runs one step per live frame once probe GI has settled.
    pub fn request_reflection_capture(&mut self, key: u64) -> bool {
        self.views.captures.request(key)
    }
    /// Cancels authored probe `key`'s requested bake before it publishes; its
    /// published image stays. False when nothing was pending.
    pub fn cancel_reflection_capture(&mut self, key: u64) -> bool {
        self.views.captures.cancel(key)
    }
    /// Authored probe `key`'s bake state, if the renderer holds its record.
    pub fn reflection_capture_status(&self, key: u64) -> Option<BakeStatus> {
        self.views.captures.status(key)
    }
    /// The published explicit captures read back since the last call (without a fence
    /// wait: each lands a frame or two after it publishes).
    pub fn take_reflection_capture_images(&mut self) -> Vec<CaptureImage> {
        std::mem::take(&mut self.views.capture_landed)
    }
    /// Restores a cached capture of authored probe `image.key` into its column (the
    /// next primary frame uploads it; a resolution that no longer matches is ignored).
    pub fn load_reflection_capture(&mut self, image: CaptureImage) -> Result<()> {
        let expected = CaptureImage::extent_for(image.resolution);
        let texels = u64::from(expected[0]) * u64::from(expected[1]);
        anyhow::ensure!(
            image.extent == expected && image.bytes.len() as u64 == texels * 8,
            "reflection capture image {:?} does not match {} texels",
            image.extent,
            image.resolution
        );
        self.views.capture_images.insert(image.key, image);
        self.views.captures.request_restore();
        Ok(())
    }
    /// A portable hash of what a capture of this scene depends on besides the probe
    /// itself: the uploaded content (and its dynamic updates), the sun, the sky and
    /// fog, and the local lights. Identical across processes for identical inputs.
    pub fn reflection_capture_inputs(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(self.scene_hash().as_bytes());
        let (d, l) = (self.content.sun_direction, self.content.sun_lux);
        for v in [d.x, d.y, d.z, l.x, l.y, l.z] {
            hash.update(v.to_le_bytes());
        }
        hash.update(format!("{:?}", (self.look.look.sky, self.look.look.fog)).as_bytes());
        hash.update(light_hash(&self.content.lights).to_le_bytes());
        format!("{:x}", hash.finalize())
    }

    /// Copies the atlas into a host-visible buffer after record `capture` published,
    /// to read its column back once the copy completes (`poll_capture_readbacks`).
    fn read_capture(&mut self, capture: usize) -> Result<()> {
        let (Some((atlas, _, state, _)), Some(size), Some((key, bake)), Some(x)) = (
            self.views.captures.atlas(),
            self.views.captures.atlas_size(),
            self.views.captures.key_and_bakes(capture),
            self.views.captures.column(capture),
        ) else {
            return Ok(());
        };
        let resolution = self.views.captures.status(key).map_or(0, |s| s.resolution);
        let buffer = self.gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 8,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("CaptureReadback".into()));
        commands.barriers([
            Barrier {
                resource: Resource::Image(atlas),
                before: state.access,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        commands.push(Command::ReadImage {
            source: atlas,
            destination: buffer,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        // Inside a live frame's deferred scope this only records the wait.
        self.gpu.resolve(point)?;
        self.views.captures.set_atlas_state(crate::graph::State {
            access: Access::CopySource,
            queue: Queue::Graphics,
        });
        self.views.capture_readbacks.push(CaptureReadback {
            key,
            bake,
            x,
            resolution,
            atlas: size,
            buffer,
            point,
        });
        Ok(())
    }

    /// Takes the column of every completed readback (all of them, waiting, when
    /// `sync`), keeps it for restores and hands it to the host.
    fn poll_capture_readbacks(&mut self, sync: bool) -> Result<()> {
        if self.views.capture_readbacks.is_empty() {
            return Ok(());
        }
        let completed = self.gpu.completed()?;
        let mut pending = Vec::new();
        for readback in std::mem::take(&mut self.views.capture_readbacks) {
            if completed[readback.point.queue.index()] < readback.point.value {
                if !sync {
                    pending.push(readback);
                    continue;
                }
                self.gpu.wait(readback.point)?;
            }
            let atlas = self.gpu.read_completed(readback.buffer, readback.point)?;
            self.gpu.retire(Resource::Buffer(readback.buffer))?;
            let extent = CaptureImage::extent_for(readback.resolution);
            let row = extent[0] as usize * 8;
            let mut bytes = Vec::with_capacity(row * extent[1] as usize);
            for y in 0..extent[1] as usize {
                let start = (y * readback.atlas[0] as usize + readback.x as usize) * 8;
                bytes.extend_from_slice(&atlas[start..start + row]);
            }
            let image = CaptureImage {
                key: readback.key,
                bake: readback.bake,
                resolution: readback.resolution,
                extent,
                bytes,
            };
            self.views.capture_images.insert(image.key, image.clone());
            self.views.capture_landed.push(image);
        }
        self.views.capture_readbacks = pending;
        Ok(())
    }

    /// Uploads the whole atlas with every explicit record's retained image (other
    /// texels zero), marking those records ready.
    fn restore_captures(&mut self) -> Result<()> {
        let (Some((atlas, _, state, _)), Some(size)) = (
            self.views.captures.atlas(),
            self.views.captures.atlas_size(),
        ) else {
            return Ok(());
        };
        let width = size[0] as usize;
        let mut texels = vec![0u8; width * size[1] as usize * 8];
        let mut restored = Vec::new();
        for (capture, key, x, resolution) in self.views.captures.explicit_columns() {
            let Some(image) = self
                .views
                .capture_images
                .get(&key)
                .filter(|i| i.resolution == resolution)
            else {
                continue;
            };
            let row = image.extent[0] as usize * 8;
            for (y, source) in image.bytes.chunks_exact(row).enumerate() {
                let start = (y * width + x as usize) * 8;
                texels[start..start + row].copy_from_slice(source);
            }
            restored.push(capture);
        }
        let staging = self.gpu.create_buffer(
            texels.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )?;
        self.gpu.upload_queued(staging, &texels)?;
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("CaptureRestore".into()));
        commands.barriers([Barrier {
            resource: Resource::Image(atlas),
            before: state.access,
            after: Access::CopyDestination,
            handoff: None,
        }]);
        commands.push(Command::UploadImage {
            source: staging,
            destination: atlas,
        });
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        self.gpu.retire(Resource::Buffer(staging))?;
        self.views.captures.set_atlas_state(crate::graph::State {
            access: Access::CopyDestination,
            queue: Queue::Graphics,
        });
        for capture in restored {
            self.views.captures.restored(capture);
        }
        Ok(())
    }

    /// Renders face `face` from `position` and projects it into the scratch column.
    fn bake_face(&mut self, face: u32, position: Vec3, resolution: u32) -> Result<()> {
        let size = [resolution; 2];
        let mut state = match self.views.capture_view.take() {
            Some(state) if state.matches_extent(size, size) => state,
            Some(old) => {
                old.destroy(&mut self.gpu)?;
                ViewState::new(self, size, size)?
            }
            None => ViewState::new(self, size, size)?,
        };
        // The surroundings as shaded (with screen-space occlusion, without its bounce),
        // minus effects that need history or several frames to land (screen-space
        // reflections, virtual shadow pages), the horizon or ray queries: probes and
        // cascades light the bake, which is the raster path's own reflection source.
        let quality = crate::look::Quality {
            reflections: None,
            ray_queries: None,
            horizon: None,
            virtual_shadows: None,
            depth_of_field: None,
            motion_blur: None,
            bloom: false,
            ..self.look.quality
        };
        let camera = CameraSpec::LookAt {
            position,
            target: position + AXES[face as usize],
            vertical_fov: std::f32::consts::FRAC_PI_2,
        };
        let result = self.with_capture_view(&mut state, quality, |renderer| {
            renderer.render_frame(&camera, 1., true, crate::capture::ViewMode::Lit, false)?;
            renderer.project_capture_face(face, resolution)
        });
        self.views.capture_view = Some(state);
        result
    }

    /// Runs `draw` with the capture camera's state swapped in, as secondary views
    /// do, sharing the probe GI; the primary's jitter override and captures stay out.
    fn with_capture_view(
        &mut self,
        state: &mut ViewState,
        quality: crate::look::Quality,
        draw: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        state.prepare_native(self)?;
        state.look.look = self.look.look;
        state.look.deferred = true;
        let mut gi = self.look.gi.take();
        let mut staging = std::mem::take(&mut self.look.probe_staging);
        let mut writes = std::mem::take(&mut self.look.probe_writes);
        state.swap(self);
        self.look.gi = gi.take();
        self.look.probe_staging = std::mem::take(&mut staging);
        self.look.probe_writes = std::mem::take(&mut writes);
        self.secondary_active = true;
        self.views.captures.baking = true;
        let jitter = self.jitter_override.take();
        let result = (|| {
            if self.look.quality != quality {
                self.set_quality(quality)?;
            }
            draw(self)
        })();
        self.jitter_override = jitter;
        self.views.captures.baking = false;
        self.secondary_active = false;
        gi = self.look.gi.take();
        staging = std::mem::take(&mut self.look.probe_staging);
        writes = std::mem::take(&mut self.look.probe_writes);
        state.swap(self);
        self.look.gi = gi;
        self.look.probe_staging = staging;
        self.look.probe_writes = writes;
        result
    }

    /// Projects the capture camera's just-lit HDR face into the scratch column
    /// (`captureFaceMain`); runs with the capture camera's state swapped in.
    fn project_capture_face(&mut self, face: u32, resolution: u32) -> Result<()> {
        let (atlas, storage, atlas_state, scratch) = self
            .views
            .captures
            .atlas()
            .context("reflection capture atlas")?;
        let host = State {
            access: Access::HostWrite,
            queue: Queue::Graphics,
        };
        let read = Access::ShaderRead;
        let mut graph = Graph::default();
        let hdr = graph.import(
            Resource::Image(self.targets.images[HDR]),
            self.targets.states[HDR],
        );
        let depth = graph.import(
            Resource::Image(self.targets.images[DEPTH]),
            self.targets.states[DEPTH],
        );
        let frame = graph.import(Resource::Buffer(self.frame), host);
        let look = graph.import(Resource::Buffer(self.look.frame), host);
        let target = graph.import(Resource::Image(atlas), atlas_state);
        // Open sky is re-evaluated from the sky view without its sun disk.
        let sky_view = crate::look::gpu::SKY_VIEW;
        let sky = self.look.images[sky_view]
            .filter(|_| self.look.look.sky.is_some())
            .map(|image| graph.import(Resource::Image(image), self.look.states[sky_view]));
        graph.pass(
            "Capture.Face",
            Queue::Graphics,
            [
                (hdr, read),
                (depth, read),
                (frame, read),
                (look, read),
                (target, Access::Storage),
            ]
            .into_iter()
            .chain(sky.map(|sky| (sky, read))),
        );
        let words = [
            self.frame_index,
            self.targets.sampled[HDR],
            storage,
            face,
            scratch,
            resolution,
            0,
            0,
        ];
        let pipeline = self.look.pipelines.capture_face;
        let groups = (resolution + 2).div_ceil(8);
        let states = self.submit_capture_graph(graph, |recorder, _| {
            recorder.push(Command::Pipeline(pipeline));
            recorder.push(Command::Constants(RootConstants { words }));
            recorder.push(Command::Dispatch([groups, groups, 1]));
        })?;
        self.targets.states[HDR] = states[hdr.0];
        self.targets.states[DEPTH] = states[depth.0];
        if let Some(sky) = sky {
            self.look.states[sky_view] = states[sky.0];
        }
        self.views.captures.set_atlas_state(states[target.0]);
        Ok(())
    }

    /// Reduces the scratch column into radiance mips and prefilters it into record
    /// `capture`'s column (`captureMipMain`, `capturePrefilterMain`).
    fn prefilter_capture(&mut self, capture: usize, resolution: u32) -> Result<()> {
        let (atlas, storage, atlas_state, scratch) = self
            .views
            .captures
            .atlas()
            .context("reflection capture atlas")?;
        let column = self
            .views
            .captures
            .column(capture)
            .context("reflection capture column")?;
        let mut graph = Graph::default();
        let target = graph.import(Resource::Image(atlas), atlas_state);
        for name in [
            "Capture.Mip1",
            "Capture.Mip2",
            "Capture.Mip3",
            "Capture.Mip4",
            "Capture.Prefilter",
        ] {
            graph.pass(name, Queue::Graphics, [(target, Access::Storage)]);
        }
        let pipelines = self.look.pipelines;
        let states = self.submit_capture_graph(graph, |recorder, pass| {
            // Passes 0..3 build scratch levels 1..4; pass 4 prefilters every level.
            let level = pass as u32 + 1;
            let (pipeline, groups) = if level < LEVELS {
                let side = ((resolution >> level) + 2).div_ceil(8);
                (pipelines.capture_mip, [side, side, 1])
            } else {
                let groups = [
                    (resolution + 2).div_ceil(8),
                    level_y(resolution, LEVELS).div_ceil(8),
                    1,
                ];
                (pipelines.capture_prefilter, groups)
            };
            recorder.push(Command::Pipeline(pipeline));
            recorder.push(Command::Constants(RootConstants {
                words: [0, 0, storage, level, scratch, resolution, column, 0],
            }));
            recorder.push(Command::Dispatch(groups));
        })?;
        self.views.captures.set_atlas_state(states[target.0]);
        Ok(())
    }

    /// Compiles and submits a bake graph; `record` records pass `n` (declaration
    /// order). Returns the final resource states.
    fn submit_capture_graph(
        &mut self,
        graph: Graph,
        mut record: impl FnMut(&mut Recorder, usize),
    ) -> Result<Vec<State>> {
        let compiled = graph.compile()?;
        let mut points = Vec::new();
        for batch in submission::batches(&compiled.passes) {
            let mut recorder = self.gpu.begin(batch[0].queue);
            for pass in batch {
                recorder.push(Command::BeginPass(pass.name.clone()));
                recorder.barriers(pass.before.iter().copied());
                record(&mut recorder, pass.id.0);
                recorder.barriers(pass.after.iter().copied());
                recorder.push(Command::EndPass);
            }
            let waits = submission::waits(batch, &points);
            let point = self.gpu.submit(recorder.finish(), &waits)?;
            points.resize(points.len() + batch.len(), point);
        }
        points.dedup();
        for point in points {
            self.gpu.resolve(point)?;
        }
        self.gpu.assert_clean()?;
        Ok(compiled.final_states)
    }
}

/// FNV-1a over the local lights, quantised to a thousandth (metres, lumens,
/// cosines): what a bake's light depends on beyond the sun.
fn light_hash(lights: &[crate::scene::Light]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for light in lights {
        let values = light
            .position_range
            .iter()
            .chain(&light.color_lumens)
            .chain(&light.direction_outer)
            .chain(&light.parameters);
        for value in values {
            for byte in ((value * 1000.).round() as i64).to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    hash
}

/// An atlas copy in flight after an explicit capture published (`read_capture`).
pub(super) struct CaptureReadback {
    key: u64,
    bake: u64,
    x: u32,
    resolution: u32,
    atlas: [u32; 2],
    buffer: ara_gpu::Buffer,
    point: TimelinePoint,
}
