//! Reflection capture bakes (`look::captures`). Each face renders through a square
//! camera of its own (a secondary view's state: targets, look, cascades and native
//! culling): deferred before the temporal resolve, unjittered, sharing the probe GI
//! and reading no captures. Compute passes then project the face into the atlas's
//! scratch column, and a capture's last face reduces and prefilters it into its
//! own column. Only primary frames bake; every camera reads the result.
use super::*;
use crate::graph::Graph;
use crate::look::captures::{CaptureSpec, LEVELS, Lighting, Settled, Work, level_y};
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
        let work = self
            .views
            .captures
            .plan(&mut self.gpu, quality, lighting, settled, sync)?;
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
                }
            }
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
