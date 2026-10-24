use crate::{GpuSystem, View};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Queue, RootConstants,
    commands::{Attachment, Command, Recorder},
};
use ara_render::graph::Graph;
use ara_vfx::{Sorting, bounds_visible, sorting, spawn_demand};

impl GpuSystem {
    pub(crate) fn run_frame(
        &mut self,
        gpu: &mut ara_gpu::vulkan::VulkanDevice,
        view: &View,
        readback: bool,
        advance: bool,
        drawn: bool,
    ) -> Result<()> {
        ensure!(self.tick < u32::MAX, "VFX tick exhausted");
        ensure!(
            view.exposure.is_finite() && view.exposure > 0.,
            "exposure must be finite and positive"
        );
        ensure!(
            !view.linear_depth || view.scene_depth.is_some(),
            "linear depth requires an imported scene depth"
        );
        ensure!(
            view.scene_background
                .is_none_or(|c| c.state.queue == Queue::Graphics),
            "scene background must have Graphics ownership"
        );
        let light = &view.lighting;
        ensure!(
            light.ambient.is_finite()
                && light.ambient.cmpge(glam::Vec3::ZERO).all()
                && light.sun_direction.is_finite()
                && light.sun_direction.length_squared() > 0.0001
                && light.sun_color.is_finite()
                && light.sun_color.cmpge(glam::Vec3::ZERO).all(),
            "invalid extracted VFX lighting"
        );
        ensure!(
            view.intensity.is_finite() && view.intensity >= 0.,
            "VFX intensity must be finite and nonnegative"
        );
        let placement = view.placement;
        let camera = placement.local_camera(&view.camera)?.resolve(self.size)?;
        self.color_target = self.hdr;
        self.color_index = self.hdr_index;
        self.preview_background = view.scene_color.is_none();
        if let Some(color) = view.scene_color {
            ensure!(
                color.state.queue == Queue::Graphics && view.scene_depth.is_some(),
                "scene HDR requires Graphics ownership and matching depth"
            );
            self.color_target = color.image;
            self.color_index = color.sampled_index;
        }
        let vp = camera.projection * camera.view;
        let inverse_view = camera.view.inverse();
        let mut frame = self.last_frame;
        frame.vp = vp;
        frame.inverse_vp = vp.inverse();
        frame.camera = camera.position.extend(1.);
        frame.right = inverse_view.x_axis;
        frame.up = inverse_view.y_axis;
        frame.forward = camera.forward.extend(0.);
        frame.extent = [
            self.size[0],
            self.size[1],
            self.layout.total,
            self.effect.emitters.len() as u32,
        ];
        frame.clock = [
            1. / self.effect.hz as f32,
            view.exposure,
            0.1,
            camera.projection.y_axis.y,
        ];
        frame.inputs = [self.config_index, self.depth_index, 1, self.material_index];
        frame.visuals = [
            self.atlas_index,
            view.scene_background.map_or(0, |c| c.sampled_index),
            u32::from(view.linear_depth)
                | if view.scene_background.is_some() {
                    2
                } else {
                    0
                },
            0,
        ];
        // ambient.w scales particle radiance; sun_direction.w converts world
        // metres (scene depth, probe lattice) into local units.
        frame.ambient = view.lighting.ambient.extend(view.intensity).to_array();
        frame.sun_direction = view
            .lighting
            .sun_direction
            .extend(placement.scale)
            .to_array();
        frame.sun_color = view.lighting.sun_color.extend(0.).to_array();
        frame.probe_counts = [0; 4];
        if let Some(p) = &view.lighting.probes {
            ensure!(
                p.state.queue == Queue::Graphics
                    && p.counts.iter().all(|c| (2..=64).contains(c))
                    && p.origin.is_finite()
                    && p.spacing.cmpgt(glam::Vec3::ZERO).all(),
                "invalid borrowed probe field"
            );
            frame.probe_origin = ((p.origin - placement.origin) / placement.scale)
                .extend(0.)
                .to_array();
            frame.probe_spacing = (p.spacing / placement.scale).extend(0.).to_array();
            frame.probe_counts = [p.counts[0], p.counts[1], p.counts[2], p.index + 1];
        }
        if let Some(depth) = view.scene_depth {
            ensure!(
                depth.state.queue == Queue::Graphics,
                "scene depth must be released to Graphics before VFX"
            );
            frame.inputs[1] = depth.sampled_index;
        }
        self.visible = 0;
        for (i, e) in self.effect.emitters.iter().enumerate() {
            let visible = u32::from(bounds_visible(e.bounds, vp));
            self.visible += visible;
            frame.demands[i] = [
                spawn_demand(e, self.tick, self.effect.hz),
                self.effect.seed,
                self.mesh_diameter(i).to_bits(),
                visible,
            ];
        }
        gpu.upload(self.frame, bytemuck::bytes_of(&frame))?;
        self.resources[4].state.access = Access::HostWrite;
        self.last_frame = frame;
        self.async_compute = view.async_compute;
        let compute = if view.async_compute {
            Queue::Compute
        } else {
            Queue::Graphics
        };
        let mut graph = Graph::default();
        let ids = self
            .resources
            .iter()
            .map(|r| graph.import(r.resource, r.state))
            .collect::<Vec<_>>();
        let scene_depth = if let Some(depth) = view.scene_depth {
            graph.import(ara_gpu::Resource::Image(depth.image), depth.state)
        } else {
            ids[8]
        };
        let scene_color = if let Some(color) = view.scene_color {
            graph.import(ara_gpu::Resource::Image(color.image), color.state)
        } else {
            ids[7]
        };
        let core = |storage| {
            vec![
                (
                    ids[0],
                    if storage {
                        Access::Storage
                    } else {
                        Access::ShaderRead
                    },
                ),
                (
                    ids[1],
                    if storage {
                        Access::Storage
                    } else {
                        Access::ShaderRead
                    },
                ),
                (
                    ids[2],
                    if storage {
                        Access::Storage
                    } else {
                        Access::Indirect
                    },
                ),
                (ids[3], Access::ShaderRead),
                (ids[4], Access::ShaderRead),
            ]
        };
        let mut acquire = core(true);
        if view.scene_depth.is_some() {
            acquire.push((scene_depth, Access::ShaderRead));
        }
        graph.pass("Vfx.Acquire", Queue::Graphics, acquire);
        let mut backdrop = vec![
            (ids[2], Access::ShaderRead),
            (ids[3], Access::ShaderRead),
            (ids[4], Access::ShaderRead),
            (scene_color, Access::Color),
            (ids[8], Access::Color),
        ];
        if let Some(background) = view.scene_background {
            backdrop.push((
                graph.import(ara_gpu::Resource::Image(background.image), background.state),
                Access::ShaderRead,
            ));
        }
        graph.pass("Vfx.Backdrop", Queue::Graphics, backdrop);
        for name in [
            "Vfx.Prepare",
            "Vfx.Spawn",
            "Vfx.Update",
            "Vfx.Compact.Scan",
            "Vfx.Compact.Prefix",
            "Vfx.Compact.Scatter",
            "Vfx.Events",
            "Vfx.Sort",
            "Vfx.IndirectArgs",
        ] {
            let mut accesses = core(true);
            // Spawn consumes the control buffer through both compute and indirect stages.
            if name == "Vfx.Spawn" {
                accesses[2].1 = Access::Indirect;
            }
            if name == "Vfx.Update" {
                accesses.push((scene_depth, Access::ShaderRead));
            }
            graph.pass(name, compute, accesses);
        }
        let mut draw = core(false);
        draw.push((ids[13], Access::ShaderRead));
        draw.push((ids[10], Access::Color));
        if let Some(p) = &view.lighting.probes {
            draw.push((
                graph.import(ara_gpu::Resource::Buffer(p.buffer), p.state),
                Access::ShaderRead,
            ));
        }
        draw.push((ids[12], Access::ShaderRead));
        for &(image, state) in &self.material_textures {
            draw.push((
                graph.import(ara_gpu::Resource::Image(image), state),
                Access::ShaderRead,
            ));
        }
        draw.extend([
            (scene_color, Access::Color),
            (scene_depth, Access::ShaderRead),
        ]);
        graph.pass("Vfx.Draw", Queue::Graphics, draw);
        graph.pass(
            "Vfx.Tonemap",
            Queue::Graphics,
            [
                (ids[4], Access::ShaderRead),
                (scene_color, Access::ShaderRead),
                (ids[9], Access::Color),
            ],
        );
        let mut copy = vec![
            (ids[2], Access::CopySource),
            (ids[5], Access::CopyDestination),
        ];
        if readback {
            copy.extend([
                (ids[9], Access::CopySource),
                (ids[6], Access::CopyDestination),
                (ids[10], Access::CopySource),
                (ids[11], Access::CopyDestination),
            ]);
        }
        graph.pass("Vfx.Readback", Queue::Graphics, copy);
        let mut host = vec![(ids[5], Access::HostRead)];
        if readback {
            host.extend([(ids[6], Access::HostRead), (ids[11], Access::HostRead)]);
        }
        graph.pass("Vfx.Host", Queue::Graphics, host);
        // Explicit stage sequence includes cosmetic event side effects and depth production.
        for p in 1..15 {
            graph.after(
                ara_render::graph::PassId(p),
                ara_render::graph::PassId(p - 1),
            )?;
        }
        let compiled = graph.compile()?;
        let mut queue = compiled.passes[0].queue;
        let mut recorder = gpu.begin(queue);
        let mut points = Vec::new();
        for pass in &compiled.passes {
            if pass.queue != queue {
                let waits = points.last().copied().into_iter().collect::<Vec<_>>();
                points.push(gpu.submit(recorder.finish(), &waits)?);
                queue = pass.queue;
                recorder = gpu.begin(queue);
            }
            recorder.barriers(pass.before.iter().copied());
            recorder.push(Command::BeginPass(pass.name.to_owned()));
            // Simulation-only ticks skip the backdrop copy, draw and tonemap.
            if (!advance && (2..=10).contains(&pass.id.0))
                || (!drawn && matches!(pass.id.0, 1 | 11 | 12))
            {
                recorder.push(Command::EndPass);
                recorder.barriers(pass.after.iter().copied());
                continue;
            }
            match pass.id.0 {
                0 | 15 => {}
                1 => self.backdrop(&mut recorder),
                2 => self.compute_stage(&mut recorder, 0),
                3 => self.compute_stage(&mut recorder, 1),
                4 => self.compute_stage(&mut recorder, 2),
                5 => self.compute_stage(&mut recorder, 3),
                6 => self.compute_stage(&mut recorder, 4),
                7 => self.compute_stage(&mut recorder, 5),
                8 => self.compute_stage(&mut recorder, 6),
                9 => self.sort(&mut recorder),
                10 => self.compute_stage(&mut recorder, 9),
                11 => self.draw(&mut recorder),
                12 => self.tonemap(&mut recorder),
                13 => {
                    recorder.push(Command::CopyBuffer {
                        source: self.control,
                        destination: self.read_control,
                        size: self.layout.configs.len() as u64 * 64,
                    });
                    if readback {
                        recorder.push(Command::ReadImage {
                            source: self.output,
                            destination: self.read_color,
                        });
                        recorder.push(Command::ReadImage {
                            source: self.feature_ids,
                            destination: self.read_ids,
                        });
                    }
                }
                14 => {}
                _ => unreachable!("all VFX graph passes are created above"),
            }
            recorder.push(Command::EndPass);
            recorder.barriers(pass.after.iter().copied());
        }
        let waits = points.last().copied().into_iter().collect::<Vec<_>>();
        points.push(gpu.submit(recorder.finish(), &waits)?);
        self.timings.clear();
        for point in points {
            self.timings.extend(gpu.resolve(point)?);
        }
        for (r, state) in self.resources.iter_mut().zip(compiled.final_states) {
            r.state = state;
        }
        if advance {
            self.tick += 1;
        }
        gpu.context().assert_clean()?;
        Ok(())
    }
    fn constants(&self, emitter: usize, k: u32, j: u32) -> Command {
        Command::Constants(RootConstants {
            words: [
                self.indices[0],
                self.indices[1],
                self.indices[2],
                self.indices[3],
                self.tick,
                emitter as u32,
                k,
                j,
            ],
        })
    }
    fn compute_stage(&self, r: &mut Recorder, stage: usize) {
        r.push(Command::Pipeline(self.pipelines.compute[stage]));
        for (i, c) in self.layout.configs.iter().enumerate() {
            if i > 0 {
                for resource in [self.particles, self.lists] {
                    r.push(Command::Barrier(ara_gpu::Barrier {
                        resource: ara_gpu::Resource::Buffer(resource),
                        before: Access::Storage,
                        after: Access::Storage,
                        handoff: None,
                    }));
                }
                if stage != 1 {
                    r.push(Command::Barrier(ara_gpu::Barrier {
                        resource: ara_gpu::Resource::Buffer(self.control),
                        before: Access::Storage,
                        after: Access::Storage,
                        handoff: None,
                    }));
                }
            }
            r.push(self.constants(i, 0, 0));
            if stage == 1 {
                r.push(Command::DispatchIndirect {
                    arguments: self.control,
                    offset: i as u64 * 64 + 32,
                });
            } else {
                r.push(Command::Dispatch([
                    if matches!(stage, 0 | 4 | 6 | 9) {
                        1
                    } else {
                        c.partition[3]
                    },
                    1,
                    1,
                ]));
            }
        }
    }
    fn sort(&self, r: &mut Recorder) {
        for (i, c) in self.layout.configs.iter().enumerate() {
            if sorting(&self.effect.emitters[i]) == Sorting::None {
                continue;
            }
            r.push(Command::Pipeline(self.pipelines.compute[7]));
            r.push(self.constants(i, 0, 0));
            r.push(Command::Dispatch([c.partition[3], 1, 1]));
            let mut k = 2;
            while k <= c.scratch[3] {
                let mut j = k / 2;
                while j > 0 {
                    r.push(Command::Barrier(ara_gpu::Barrier {
                        resource: ara_gpu::Resource::Buffer(self.lists),
                        before: Access::Storage,
                        after: Access::Storage,
                        handoff: None,
                    }));
                    r.push(Command::Pipeline(self.pipelines.compute[8]));
                    r.push(self.constants(i, k, j));
                    r.push(Command::Dispatch([c.partition[3], 1, 1]));
                    j /= 2;
                }
                k *= 2;
            }
        }
    }
    fn backdrop(&self, r: &mut Recorder) {
        if !self.preview_background {
            return;
        }
        r.push(Command::Pipeline(self.pipelines.backdrop));
        r.push(self.constants(0, 0, 0));
        r.push(Command::BeginRendering {
            colors: vec![
                Attachment {
                    image: self.hdr,
                    clear: Some([0., 0., 0., 1.]),
                },
                Attachment {
                    image: self.depth,
                    clear: Some([0.; 4]),
                },
            ],
            depth: None,
            size: self.size,
        });
        r.push(Command::Fullscreen);
        r.push(Command::EndRendering);
    }
    fn draw(&self, r: &mut Recorder) {
        r.push(Command::Pipeline(self.pipelines.particles));
        r.push(Command::BeginRendering {
            colors: vec![
                Attachment {
                    image: self.color_target,
                    clear: None,
                },
                Attachment {
                    image: self.feature_ids,
                    clear: Some([0.; 4]),
                },
            ],
            depth: None,
            size: self.size,
        });
        for i in 0..self.layout.configs.len() {
            if self.last_frame.demands[i][3] != 0 {
                r.push(self.constants(i, 0, 0));
                r.push(Command::DrawIndirect {
                    arguments: self.control,
                    offset: i as u64 * 64 + 16,
                });
            }
        }
        r.push(Command::EndRendering);
    }
    fn tonemap(&self, r: &mut Recorder) {
        r.push(Command::Pipeline(self.pipelines.tonemap));
        r.push(self.constants(0, self.color_index, 0));
        r.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: self.output,
                clear: Some([0.; 4]),
            }],
            depth: None,
            size: self.size,
        });
        r.push(Command::Fullscreen);
        r.push(Command::EndRendering);
    }
}
