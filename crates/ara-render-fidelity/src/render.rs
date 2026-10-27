use crate::{pipeline_variants::Transport, settings::*, targets::*, view::FidelityView};
use anyhow::Result;
use ara_gpu::{Access, GpuDevice, Queue, Resource, RootConstants, vulkan::Command};
use ara_render::graph::{Graph, State};

impl FidelityView {
    /// Execute graph-declared passes and publish a completed offscreen output.
    /// No compiler, image readback or wall-clock sampling runs in this method.
    pub fn render(&mut self, gpu: &mut GpuDevice, input: &FrameInput<'_>) -> Result<()> {
        if self.shader_epoch != gpu.shader_epoch() {
            self.shader_epoch = gpu.shader_epoch();
            let directory = ara_gpu::cooked::resource_dir(
                env!("CARGO_PKG_NAME"),
                concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
            );
            if let Some(key) = gpu.shader_status().cohorts.get(&directory) {
                self.shader_revision = key.clone();
            }
            self.history = false;
            self.samples = 0;
            self.atmosphere_dirty = true;
        }
        anyhow::ensure!(
            self.targets.images.len() >= 28,
            "view requires a successful target resize"
        );
        self.prepare_transport(gpu)?;
        let (frame, vp, camera) = self.frame(input)?;
        gpu.upload(self.frame_buffer, bytemuck::bytes_of(&frame))?;
        let mut graph = Graph::default();
        let ids = self
            .targets
            .images
            .iter()
            .zip(&self.targets.states)
            .map(|(i, s)| graph.import(Resource::Image(*i), *s))
            .collect::<Vec<_>>();
        let probes =
            [0, 1].map(|i| graph.import(Resource::Buffer(self.probes[i]), self.probe_states[i]));
        let exposure = graph.import(Resource::Buffer(self.exposure), self.exposure_state);
        let shadow = match self.shadow_source {
            crate::SunShadowSource::RayQueries => None,
            crate::SunShadowSource::VirtualShadowMap { visibility, .. } => {
                Some(graph.import(Resource::Image(visibility.image), visibility.state))
            }
        };
        let reservoir_ids = self.reservoirs.as_ref().map(|r| {
            std::array::from_fn::<_, 6, _>(|i| {
                graph.import(Resource::Buffer(r.buffers[i]), r.states[i])
            })
        });
        let mut shared = self
            .scene
            .resources()
            .into_iter()
            .map(|(r, a)| {
                (
                    graph.import(
                        r,
                        State {
                            access: a,
                            queue: Queue::Graphics,
                        },
                    ),
                    a,
                )
            })
            .collect::<Vec<_>>();
        let f = graph.import(
            Resource::Buffer(self.frame_buffer),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        shared.push((f, Access::ShaderRead));
        shared.push((
            graph.import(
                Resource::Image(self.sheen_lut),
                State {
                    access: Access::ShaderRead,
                    queue: Queue::Graphics,
                },
            ),
            Access::ShaderRead,
        ));
        let lut = graph.import(
            Resource::Image(self.lut),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let mut recipes = Vec::new();
        let render = self.settings.render_size;
        let display = self.settings.display_size;
        let dispatch = [render[0].div_ceil(8), render[1].div_ceil(8), 1];
        let mut add = |name: &str,
                       pipeline: &'static str,
                       arg: u32,
                       groups: [u32; 3],
                       uses: Vec<_>|
         -> Result<()> {
            let id = graph.pass(name, Queue::Graphics, uses);
            graph.accesses(id, shared.iter().copied())?;
            if pipeline.starts_with("primary")
                && let Some(shadow) = shadow
            {
                graph.accesses(id, [(shadow, Access::ShaderRead)])?;
            }
            if self.atmosphere.is_some()
                && (pipeline.starts_with("primary")
                    || pipeline.starts_with("hybrid")
                    || pipeline.starts_with("reference")
                    || pipeline.starts_with("reservoir")
                    || pipeline.starts_with("ddgi"))
            {
                graph.accesses(
                    id,
                    [
                        (ids[SKYVIEW], Access::ShaderRead),
                        (ids[TRANSMITTANCE], Access::ShaderRead),
                    ],
                )?;
            }
            recipes.push((id, pipeline, arg, groups));
            Ok(())
        };
        let slot = self.slot;
        let previous = 1 - slot;
        let reset = input.reset || !self.history;
        let lighting_response = !reset && (input.lighting_changed || self.responsive_frames > 0);
        let rt = self.settings.tier == Tier::Rt;
        let reference = matches!(self.settings.mode, Mode::Reference { .. });
        let resampling = reservoir_ids.is_some() && rt && !reference;
        if self.atmosphere.is_some() {
            if self.atmosphere_dirty {
                add(
                    "Atmosphere.Transmittance",
                    "transmittance",
                    0,
                    [32, 8, 1],
                    vec![(ids[TRANSMITTANCE], Access::Storage)],
                )?;
                add(
                    "Atmosphere.MultiScatter",
                    "multiscatter",
                    0,
                    [4, 4, 1],
                    vec![
                        (ids[TRANSMITTANCE], Access::ShaderRead),
                        (ids[MULTISCATTER], Access::Storage),
                    ],
                )?;
            }
            add(
                "Atmosphere.SkyView",
                "skyview",
                0,
                [24, 14, 1],
                vec![
                    (ids[TRANSMITTANCE], Access::ShaderRead),
                    (ids[MULTISCATTER], Access::ShaderRead),
                    (ids[SKYVIEW], Access::Storage),
                ],
            )?;
            add(
                "Atmosphere.Froxels.AerialPerspective",
                if rt { "fog" } else { "fog_raster" },
                0,
                [8, 5, 1],
                vec![
                    (ids[TRANSMITTANCE], Access::ShaderRead),
                    (ids[MULTISCATTER], Access::ShaderRead),
                    (ids[FROXELS], Access::Storage),
                ],
            )?;
        }
        add(
            "Visibility.Motion",
            self.transport_pipeline(Transport::Primary),
            0,
            dispatch,
            [
                WORLD0 + slot,
                NORMAL0 + slot,
                BASE,
                MOTION,
                DEPTH,
                DIRECT,
                RAW,
            ]
            .map(|i| (ids[i], Access::Storage))
            .into_iter()
            .chain(if resampling {
                vec![
                    (ids[SPEC_RAW], Access::Storage),
                    (ids[SURFACE_ALBEDO], Access::Storage),
                    (ids[SURFACE_GEOMETRY], Access::Storage),
                ]
            } else {
                vec![]
            })
            .collect(),
        )?;
        if rt && !reference {
            add(
                "DDGI.Trace.Relocate.Classify.Integrate",
                self.transport_pipeline(Transport::Probes),
                0,
                [self.volume.len() as u32, 1, 1],
                vec![
                    (probes[1 - self.probe_slot], Access::ShaderRead),
                    (probes[self.probe_slot], Access::Storage),
                ],
            )?;
        }
        if let Some(r) = reservoir_ids.filter(|_| resampling) {
            let mut guides = vec![
                (ids[WORLD0 + slot], Access::ShaderRead),
                (ids[NORMAL0 + slot], Access::ShaderRead),
                (ids[BASE], Access::ShaderRead),
                (ids[MOTION], Access::ShaderRead),
                (ids[SURFACE_ALBEDO], Access::ShaderRead),
                (ids[SURFACE_GEOMETRY], Access::ShaderRead),
                (probes[self.probe_slot], Access::ShaderRead),
            ];
            if !reset {
                guides.extend(
                    [
                        WORLD0 + previous,
                        NORMAL0 + previous,
                        DIRECT_HISTORY0 + previous,
                    ]
                    .map(|i| (ids[i], Access::ShaderRead)),
                );
            }
            let mut init = guides.to_vec();
            init.extend([
                (ids[DIRECT], Access::ShaderRead),
                (r[2], Access::Storage),
                (r[5], Access::Storage),
                (ids[SPEC_RAW], Access::Storage),
            ]);
            if !reset {
                init.extend([
                    (r[previous], Access::ShaderRead),
                    (r[3 + previous], Access::ShaderRead),
                ]);
            }
            add(
                "ReSTIR.Candidates.Temporal",
                if self.atmosphere.is_some() {
                    "reservoir_init_atmosphere"
                } else if self.scene.has_layers {
                    "reservoir_init_layers"
                } else {
                    "reservoir_init"
                },
                0,
                [render[0].div_ceil(16), render[1].div_ceil(16), 1],
                init,
            )?;
            let mut trace = guides.to_vec();
            trace.extend([
                (r[5], Access::Storage),
                (ids[SPEC_RAW], Access::Storage),
                (exposure, Access::ShaderRead),
            ]);
            add(
                "ReSTIR.GI.CompactTrace",
                if self.atmosphere.is_some() {
                    "reservoir_trace_atmosphere"
                } else if self.scene.has_layers {
                    "reservoir_trace_layers"
                } else {
                    "reservoir_trace"
                },
                0,
                if reset || lighting_response {
                    [render[0].div_ceil(16), render[1].div_ceil(16), 1]
                } else {
                    [
                        render[0].div_ceil(16 * (self.transport_stride / 2).max(1)),
                        render[1].div_ceil(16),
                        1,
                    ]
                },
                trace,
            )?;
            let mut specular = guides.to_vec();
            specular.extend([
                (ids[SPEC_RAW], Access::Storage),
                (exposure, Access::ShaderRead),
            ]);
            add(
                "ReSTIR.Specular.Dense",
                if self.atmosphere.is_some() {
                    "reservoir_specular_atmosphere"
                } else if self.scene.has_layers {
                    "reservoir_specular_layers"
                } else {
                    "reservoir_specular"
                },
                0,
                dispatch,
                specular,
            )?;
            // Spatial reuse and visibility resolve one reservoir per 2x2 grid block;
            // the shade pass lights every pixel from the four nearest blocks.
            let mut spatial = guides.to_vec();
            spatial.extend([
                (r[2], Access::ShaderRead),
                (r[5], Access::ShaderRead),
                (r[slot], Access::Storage),
                (r[3 + slot], Access::Storage),
            ]);
            add(
                "ReSTIR.Spatial.Visibility",
                if self.atmosphere.is_some() {
                    "reservoir_spatial_atmosphere"
                } else if self.scene.has_layers {
                    "reservoir_spatial_layers"
                } else {
                    "reservoir_spatial"
                },
                0,
                [render[0].div_ceil(16), render[1].div_ceil(16), 1],
                spatial,
            )?;
            guides.extend([
                (r[slot], Access::ShaderRead),
                (r[3 + slot], Access::ShaderRead),
                (ids[RAW], Access::Storage),
                (ids[DIRECT], Access::Storage),
                (ids[SPEC_RAW], Access::Storage),
            ]);
            add(
                "ReSTIR.Shade",
                if self.atmosphere.is_some() {
                    "reservoir_shade_atmosphere"
                } else if self.scene.has_layers {
                    "reservoir_shade_layers"
                } else {
                    "reservoir_shade"
                },
                0,
                dispatch,
                guides,
            )?;
        }
        let mut transport_history = Vec::new();
        if !reset && !reference {
            transport_history.extend(
                [
                    WORLD0 + previous,
                    NORMAL0 + previous,
                    DIRECT_HISTORY0 + previous,
                ]
                .map(|i| (ids[i], Access::ShaderRead)),
            );
        }
        if !resampling {
            add(
                if reference {
                    "Reference.PathTransport"
                } else {
                    "RT.Reflections.SoftShadows.DDGI"
                },
                self.transport_pipeline(if reference {
                    Transport::Reference
                } else {
                    Transport::Gather
                }),
                if !reference && !reset && !lighting_response {
                    if self.transport_stride == 4 { 3 } else { 1 }
                } else {
                    0
                },
                if !reference && !reset && !lighting_response {
                    [
                        render[0].div_ceil(8 * self.transport_stride),
                        render[1].div_ceil(8),
                        1,
                    ]
                } else {
                    dispatch
                },
                vec![
                    (ids[RAW], Access::Storage),
                    (ids[DIRECT], Access::ShaderRead),
                    (ids[WORLD0 + slot], Access::ShaderRead),
                    (ids[NORMAL0 + slot], Access::ShaderRead),
                    (ids[MOTION], Access::ShaderRead),
                    (ids[BASE], Access::ShaderRead),
                    (probes[self.probe_slot], Access::ShaderRead),
                ]
                .into_iter()
                .chain(transport_history)
                .collect(),
            )?;
        }
        if !resampling && !reference && !reset && !lighting_response && frame.transport[3] == 0 {
            add(
                "RT.Disocclusion.Repair",
                self.transport_pipeline(Transport::Gather),
                if self.transport_stride == 4 { 4 } else { 2 },
                [
                    render[0].div_ceil(if self.transport_stride == 4 { 8 } else { 16 }),
                    render[1].div_ceil(8),
                    1,
                ],
                vec![
                    (ids[RAW], Access::Storage),
                    (ids[DIRECT], Access::ShaderRead),
                    (ids[WORLD0 + slot], Access::ShaderRead),
                    (ids[NORMAL0 + slot], Access::ShaderRead),
                    (ids[WORLD0 + previous], Access::ShaderRead),
                    (ids[NORMAL0 + previous], Access::ShaderRead),
                    (ids[DIRECT_HISTORY0 + previous], Access::ShaderRead),
                    (ids[MOTION], Access::ShaderRead),
                    (ids[BASE], Access::ShaderRead),
                    (probes[self.probe_slot], Access::ShaderRead),
                ],
            )?;
        }
        let mut temporal = vec![
            (ids[RAW], Access::ShaderRead),
            (ids[DIRECT], Access::ShaderRead),
            (ids[DIRECT_HISTORY0 + slot], Access::Storage),
            (ids[WORLD0 + slot], Access::ShaderRead),
            (ids[NORMAL0 + slot], Access::ShaderRead),
            (ids[MOTION], Access::ShaderRead),
            (ids[HISTORY0 + slot], Access::Storage),
            (ids[MOMENTS0 + slot], Access::Storage),
        ];
        if !reset {
            temporal.extend(
                [
                    WORLD0 + previous,
                    NORMAL0 + previous,
                    HISTORY0 + previous,
                    DIRECT_HISTORY0 + previous,
                    MOMENTS0 + previous,
                ]
                .map(|i| (ids[i], Access::ShaderRead)),
            );
        }
        if resampling {
            temporal.extend([
                (ids[SPEC_RAW], Access::ShaderRead),
                (ids[SPEC_HISTORY0 + slot], Access::Storage),
                (ids[SPEC_MOMENTS0 + slot], Access::Storage),
            ]);
            if !reset {
                temporal.extend([
                    (ids[SPEC_HISTORY0 + previous], Access::ShaderRead),
                    (ids[SPEC_MOMENTS0 + previous], Access::ShaderRead),
                ]);
            }
        }
        add(
            if reference {
                "Reference.Accumulate.Undenoised"
            } else {
                "SVGF.Reproject.Reject.Moments"
            },
            if resampling {
                "temporal_split"
            } else {
                "temporal"
            },
            0,
            dispatch,
            temporal,
        )?;
        // Bounded reservoir budgets skip the finest wavelet; pass 1 reads history.
        let first = usize::from(resampling && self.bounded_reservoirs);
        for i in first..if reference { 1 } else { 3 } {
            let source = if i == first {
                HISTORY0 + slot
            } else if i == 1 {
                FILTER0
            } else {
                FILTER1
            };
            let target = if i == 1 { FILTER1 } else { FILTER0 };
            let mut uses = vec![
                (ids[source], Access::ShaderRead),
                (ids[target], Access::Storage),
            ];
            uses.extend(
                [
                    BASE,
                    DEPTH,
                    WORLD0 + slot,
                    NORMAL0 + slot,
                    MOMENTS0 + slot,
                    DIRECT_HISTORY0 + slot,
                ]
                .map(|j| (ids[j], Access::ShaderRead)),
            );
            if resampling {
                let source = if i == first {
                    SPEC_HISTORY0 + slot
                } else if i == 1 {
                    SPEC_FILTER0
                } else {
                    SPEC_FILTER1
                };
                let target = if i == 1 { SPEC_FILTER1 } else { SPEC_FILTER0 };
                uses.extend([
                    (ids[source], Access::ShaderRead),
                    (ids[target], Access::Storage),
                    (ids[SPEC_MOMENTS0 + slot], Access::ShaderRead),
                ]);
            }
            add(
                if reference {
                    "Reference.Copy"
                } else {
                    if resampling {
                        ["SVGF.Atrous.1", "SVGF.Atrous.3", "SVGF.Atrous.8"][i]
                    } else {
                        ["SVGF.Atrous.1", "SVGF.Atrous.2", "SVGF.Atrous.4"][i]
                    }
                },
                if resampling { "atrous_split" } else { "atrous" },
                i as u32,
                dispatch,
                uses,
            )?;
        }
        add(
            "Exposure.EV100.Meter.Adapt",
            "exposure",
            0,
            [1, 1, 1],
            vec![
                (ids[FILTER0], Access::ShaderRead),
                (exposure, Access::Storage),
            ],
        )?;
        // With deferred post, the feature composite's `post_process` produces the
        // only output of this frame (see `set_deferred_post`).
        if !self.deferred_post {
            for (name, pass, groups, uses) in bloom_passes(
                &BLOOM_NAMES,
                render,
                ids[FILTER0],
                [ids[BLOOM0], ids[BLOOM1]],
                exposure,
            ) {
                add(name, "bloom", pass, groups, uses)?;
            }
            add(
                "Post.Atmosphere.Lens.ToneMap.LUT",
                "post",
                0,
                dispatch,
                vec![
                    (ids[FILTER0], Access::ShaderRead),
                    (ids[BLOOM1], Access::ShaderRead),
                    (ids[DEPTH], Access::ShaderRead),
                    (ids[MOTION], Access::ShaderRead),
                    (ids[POST], Access::Storage),
                    (exposure, Access::ShaderRead),
                    (lut, Access::ShaderRead),
                ]
                .into_iter()
                .chain(if self.atmosphere.is_some() {
                    vec![
                        (ids[SKYVIEW], Access::ShaderRead),
                        (ids[TRANSMITTANCE], Access::ShaderRead),
                        (ids[FROXELS], Access::ShaderRead),
                        (ids[WORLD0 + slot], Access::ShaderRead),
                    ]
                } else {
                    vec![]
                })
                .collect(),
            )?;
            let fsr = u32::from(self.settings.upscaler == Upscaler::Fsr1 && render != display);
            let display_groups = [display[0].div_ceil(8), display[1].div_ceil(8), 1];
            let mut upscale = vec![
                (ids[POST], Access::ShaderRead),
                (ids[UPSCALED], Access::Storage),
                (ids[DISPLAY_HISTORY0 + slot], Access::Storage),
                (ids[MOTION], Access::ShaderRead),
                (ids[WORLD0 + slot], Access::ShaderRead),
                (ids[DEPTH], Access::ShaderRead),
                (exposure, Access::ShaderRead),
            ];
            if !reset && fsr == 0 {
                upscale.extend(
                    [DISPLAY_HISTORY0 + previous, WORLD0 + previous]
                        .map(|i| (ids[i], Access::ShaderRead)),
                );
            }
            add(
                if fsr == 1 {
                    "Upscale.FSR1.EASU"
                } else {
                    "Upscale.TAAU.Resolve"
                },
                "upscale",
                fsr,
                display_groups,
                upscale,
            )?;
            add(
                if fsr == 1 {
                    "Upscale.FSR1.RCAS"
                } else {
                    "Output.SDR"
                },
                "sharpen",
                fsr,
                display_groups,
                vec![
                    (ids[UPSCALED], Access::ShaderRead),
                    (ids[OUTPUT], Access::Storage),
                ],
            )?;
        }
        let ready = graph.pass(
            "Viewport.Ready",
            Queue::Graphics,
            [if self.deferred_post {
                (ids[FILTER0], Access::ShaderRead)
            } else {
                (ids[OUTPUT], Access::CopySource)
            }],
        );
        let compiled = graph.compile()?;
        let mut points = Vec::new();
        for pass in &compiled.passes {
            let mut c = gpu.begin(pass.queue);
            c.push(Command::BeginPass(pass.name.to_owned()));
            c.barriers(pass.before.iter().copied());
            if pass.id != ready {
                let (_, name, arg, groups) = recipes
                    .iter()
                    .find(|(id, _, _, _)| *id == pass.id)
                    .ok_or_else(|| anyhow::anyhow!("Missing fidelity pass recipe"))?;
                c.push(Command::Pipeline(self.pipelines[name]));
                c.push(Command::Constants(RootConstants {
                    words: [self.frame_index, *arg, 0, 0, 0, 0, 0, 0],
                }));
                c.push(Command::Dispatch(*groups));
            }
            c.barriers(pass.after.iter().copied());
            c.push(Command::EndPass);
            let waits = pass.waits.iter().map(|i| points[*i]).collect::<Vec<_>>();
            points.push(gpu.submit(c.finish(), &waits)?);
        }
        self.timings.clear();
        for p in points {
            self.timings.extend(gpu.resolve(p)?);
        }
        for (i, id) in ids.iter().enumerate() {
            self.targets.states[i] = compiled.final_states[id.0];
        }
        for (i, id) in probes.iter().enumerate() {
            self.probe_states[i] = compiled.final_states[id.0];
        }
        self.exposure_state = compiled.final_states[exposure.0];
        if let (Some(r), Some(ids)) = (&mut self.reservoirs, reservoir_ids) {
            for (i, id) in ids.iter().enumerate() {
                r.states[i] = compiled.final_states[id.0];
            }
        }
        if let Mode::Reference { samples, .. } = self.settings.mode {
            self.samples = if reset { 0 } else { self.samples };
            self.samples = (self.samples + 8).min(samples);
        }
        self.previous_jitter = glam::Vec2::new(frame.jitter[0], frame.jitter[1]);
        self.previous_vp = vp;
        self.previous_camera = camera;
        self.history = true;
        self.last_frame = Some(frame);
        self.atmosphere_dirty = false;
        self.slot = previous;
        self.responsive_frames = if input.lighting_changed {
            7
        } else {
            self.responsive_frames.saturating_sub(1)
        };
        self.probe_start = (self.probe_start
            + self.settings.probe_ray_budget / if lighting_response { 16 } else { 64 })
            % self.volume.len() as u32;
        self.probe_slot = 1 - self.probe_slot;
        gpu.assert_clean()?;
        self.scene.presented();
        Ok(())
    }
}
