use super::*;
use crate::{capture::ViewMode, graph::Graph};
use ara_gpu::{
    RootConstants,
    vulkan::{Attachment, Command, Draw},
};

impl Renderer {
    pub(super) fn render_frame(
        &mut self,
        camera_spec: &CameraSpec,
        exposure: f32,
        deterministic: bool,
        mode: ViewMode,
    ) -> Result<()> {
        ensure!(
            exposure.is_finite() && exposure > 0.,
            "Exposure must be a finite positive multiplier"
        );
        let camera = camera_spec.resolve(self.size)?;
        let output_history = if self.targets.history == Some(HISTORY0) {
            HISTORY1
        } else {
            HISTORY0
        };
        let frame = self.frame_data(&camera, exposure, deterministic, mode, output_history);
        self.gpu.upload(self.frame, bytemuck::bytes_of(&frame))?;
        let mut compute_frame = frame;
        compute_frame.lighting[3] = self.content.compute_light_index;
        self.gpu
            .upload(self.compute_frame, bytemuck::bytes_of(&compute_frame))?;
        let mut graph = Graph::default();
        let ids = self
            .targets
            .images
            .iter()
            .zip(&self.targets.states)
            .map(|(&image, &state)| graph.import(Resource::Image(image), state))
            .collect::<Vec<_>>();
        let tile = graph.import(
            Resource::Buffer(self.targets.tiles),
            State {
                access: Access::Storage,
                queue: Queue::Compute,
            },
        );
        graph.pass(
            "ShadowAtlas.4Cascades",
            Queue::Graphics,
            [(ids[SHADOW], Access::Depth)],
        );
        graph.pass(
            "GBuffer",
            Queue::Graphics,
            [
                (ids[ALBEDO], Access::Color),
                (ids[NORMAL], Access::Color),
                (ids[MATERIAL], Access::Color),
                (ids[WORLD], Access::Color),
                (ids[ID], Access::Color),
                (ids[DEPTH], Access::Depth),
            ],
        );
        graph.pass("TiledLightCull", Queue::Compute, [(tile, Access::Storage)]);
        graph.pass(
            "DeferredLighting",
            Queue::Graphics,
            [
                (ids[ALBEDO], Access::ShaderRead),
                (ids[NORMAL], Access::ShaderRead),
                (ids[MATERIAL], Access::ShaderRead),
                (ids[WORLD], Access::ShaderRead),
                (ids[SHADOW], Access::ShaderRead),
                (tile, Access::ShaderRead),
                (ids[HDR], Access::Color),
            ],
        );
        graph.pass(
            "ForwardTransparency",
            Queue::Graphics,
            [
                (ids[HDR], Access::Color),
                (ids[ALBEDO], Access::Color),
                (ids[NORMAL], Access::Color),
                (ids[MATERIAL], Access::Color),
                (ids[WORLD], Access::Color),
                (ids[DEPTH], Access::Depth),
                (ids[ID], Access::Color),
                (ids[SHADOW], Access::ShaderRead),
                (tile, Access::ShaderRead),
            ],
        );
        let mut taa = vec![
            (ids[HDR], Access::ShaderRead),
            (ids[WORLD], Access::ShaderRead),
            (ids[DEPTH], Access::ShaderRead),
            (ids[output_history], Access::Storage),
        ];
        if let Some(history) = self.targets.history {
            taa.push((ids[history], Access::ShaderRead));
        }
        graph.pass("TemporalResolve", Queue::Graphics, taa);
        graph.pass(
            "Exposure.ACESFitted.SDR",
            Queue::Graphics,
            [
                (ids[output_history], Access::ShaderRead),
                (ids[OUTPUT], Access::Storage),
                (ids[ALBEDO], Access::ShaderRead),
                (ids[NORMAL], Access::ShaderRead),
                (ids[MATERIAL], Access::ShaderRead),
                (ids[WORLD], Access::ShaderRead),
                (ids[ID], Access::ShaderRead),
                (ids[DEPTH], Access::ShaderRead),
                (ids[SHADOW], Access::ShaderRead),
            ],
        );
        graph.pass(
            "PresentationReady",
            Queue::Graphics,
            [
                (ids[OUTPUT], Access::CopySource),
                (ids[ID], Access::CopySource),
            ],
        );
        graph.pass(
            "TileOwnershipReturn",
            Queue::Compute,
            [(tile, Access::Storage)],
        );
        let mut graphics = self
            .content
            .static_inputs
            .iter()
            .chain(&self.content.geometry_inputs)
            .map(|&(resource, access)| {
                let id = graph.import(
                    resource,
                    State {
                        access,
                        queue: Queue::Graphics,
                    },
                );
                (id, access)
            })
            .collect::<Vec<_>>();
        let frame_id = graph.import(
            Resource::Buffer(self.frame),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        graphics.push((frame_id, Access::ShaderRead));
        // Host-written storage: dynamic vertices and the light list.
        let host_write = State {
            access: Access::HostWrite,
            queue: Queue::Graphics,
        };
        for mesh in self.content.meshes.iter().filter(|m| m.host) {
            let id = graph.import(Resource::Buffer(mesh.vertices), host_write);
            graphics.push((id, Access::Vertex));
        }
        let lights = graph.import(Resource::Buffer(self.content.light_buffer), host_write);
        graphics.push((lights, Access::ShaderRead));
        for pass in [0, 1, 3, 4, 5, 6] {
            graph.accesses(crate::graph::PassId(pass), graphics.iter().copied())?;
        }
        let compute_lights = graph.import(
            Resource::Buffer(self.content.compute_lights),
            State {
                access: Access::HostWrite,
                queue: Queue::Compute,
            },
        );
        let mut compute = vec![(compute_lights, Access::ShaderRead)];
        let compute_frame = graph.import(
            Resource::Buffer(self.compute_frame),
            State {
                access: Access::HostWrite,
                queue: Queue::Compute,
            },
        );
        compute.push((compute_frame, Access::ShaderRead));
        graph.accesses(crate::graph::PassId(2), compute)?;
        let compiled = graph.compile()?;
        let mut points = Vec::new();
        let root = RootConstants {
            words: [
                self.frame_index,
                self.content.material_index,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        };
        for pass in &compiled.passes {
            let mut recorder = self.gpu.begin(pass.queue);
            recorder.push(Command::BeginPass(pass.name.clone()));
            recorder.barriers(pass.before.iter().copied());
            match pass.id.0 {
                0 => {
                    recorder.push(Command::Pipeline(self.pipelines.shadow));
                    recorder.push(Command::BeginRendering {
                        colors: Vec::new(),
                        depth: Some(Attachment {
                            image: self.targets.images[SHADOW],
                            clear: Some([0.; 4]),
                        }),
                        size: [1024, 1024],
                    });
                    for cascade in 0..4 {
                        recorder.push(Command::Viewport {
                            origin: [cascade % 2 * 512, cascade / 2 * 512],
                            size: [512, 512],
                        });
                        for (index, mesh) in self
                            .content
                            .meshes
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| !m.transparent)
                        {
                            let mut constants = root;
                            constants.words[2] = mesh.material;
                            constants.words[3] = index as u32 + 1;
                            constants.words[4] = cascade;
                            recorder.push(Command::Constants(constants));
                            recorder.push(Command::Draw(draw(mesh)));
                        }
                    }
                    recorder.push(Command::EndRendering);
                }
                1 => {
                    recorder.push(Command::Pipeline(self.pipelines.gbuffer));
                    recorder.push(Command::BeginRendering {
                        colors: [ALBEDO, NORMAL, MATERIAL, WORLD, ID]
                            .iter()
                            .map(|&i| Attachment {
                                image: self.targets.images[i],
                                clear: Some([0.; 4]),
                            })
                            .collect(),
                        depth: Some(Attachment {
                            image: self.targets.images[DEPTH],
                            clear: Some([0.; 4]),
                        }),
                        size: self.size,
                    });
                    for (index, mesh) in self
                        .content
                        .meshes
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| !m.transparent)
                    {
                        let mut constants = root;
                        constants.words[2] = mesh.material;
                        constants.words[3] = index as u32 + 1;
                        recorder.push(Command::Constants(constants));
                        recorder.push(Command::Draw(draw(mesh)));
                    }
                    recorder.push(Command::EndRendering);
                }
                2 => {
                    recorder.push(Command::Pipeline(self.pipelines.tile));
                    let mut constants = root;
                    constants.words[0] = self.compute_frame_index;
                    recorder.push(Command::Constants(constants));
                    recorder.push(Command::Dispatch([
                        (self.size[0].div_ceil(16) * self.size[1].div_ceil(16)).div_ceil(64),
                        1,
                        1,
                    ]));
                }
                3 => {
                    recorder.push(Command::Pipeline(self.pipelines.lighting));
                    recorder.push(Command::Constants(root));
                    recorder.push(Command::BeginRendering {
                        colors: vec![Attachment {
                            image: self.targets.images[HDR],
                            clear: Some([0.; 4]),
                        }],
                        depth: None,
                        size: self.size,
                    });
                    recorder.push(Command::Fullscreen);
                    recorder.push(Command::EndRendering);
                }
                4 => {
                    recorder.push(Command::Pipeline(self.pipelines.forward));
                    recorder.push(Command::BeginRendering {
                        colors: vec![
                            Attachment {
                                image: self.targets.images[HDR],
                                clear: None,
                            },
                            Attachment {
                                image: self.targets.images[ID],
                                clear: None,
                            },
                            Attachment {
                                image: self.targets.images[ALBEDO],
                                clear: None,
                            },
                            Attachment {
                                image: self.targets.images[NORMAL],
                                clear: None,
                            },
                            Attachment {
                                image: self.targets.images[MATERIAL],
                                clear: None,
                            },
                            Attachment {
                                image: self.targets.images[WORLD],
                                clear: None,
                            },
                        ],
                        depth: Some(Attachment {
                            image: self.targets.images[DEPTH],
                            clear: None,
                        }),
                        size: self.size,
                    });
                    let mut transparent = self
                        .content
                        .meshes
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.transparent)
                        .collect::<Vec<_>>();
                    transparent.sort_by(|(_, a), (_, b)| {
                        b.centre
                            .distance_squared(camera.position)
                            .total_cmp(&a.centre.distance_squared(camera.position))
                    });
                    for (index, mesh) in transparent {
                        let mut constants = root;
                        constants.words[2] = mesh.material;
                        constants.words[3] = index as u32 + 1;
                        recorder.push(Command::Constants(constants));
                        recorder.push(Command::Draw(draw(mesh)));
                    }
                    recorder.push(Command::EndRendering);
                }
                5 | 6 => {
                    recorder.push(Command::Pipeline(if pass.id.0 == 5 {
                        self.pipelines.taa
                    } else {
                        self.pipelines.tonemap
                    }));
                    let mut constants = root;
                    if pass.id.0 == 6 {
                        constants.words[1] = self.targets.sampled[output_history];
                    }
                    recorder.push(Command::Constants(constants));
                    recorder.push(Command::Dispatch([
                        self.size[0].div_ceil(8),
                        self.size[1].div_ceil(8),
                        1,
                    ]));
                }
                _ => {}
            }
            recorder.barriers(pass.after.iter().copied());
            recorder.push(Command::EndPass);
            let waits = pass.waits.iter().map(|i| points[*i]).collect::<Vec<_>>();
            points.push(self.gpu.submit(recorder.finish(), &waits)?);
        }
        self.last_timings.clear();
        for point in points {
            self.last_timings.extend(self.gpu.resolve(point)?);
        }
        for (i, id) in ids.iter().enumerate() {
            self.targets.states[i] = compiled.final_states[id.0];
        }
        self.targets.history = Some(output_history);
        self.previous_vp = frame.vp;
        self.frame_number = self.frame_number.wrapping_add(1);
        self.gpu.assert_clean()?;
        Ok(())
    }
}
fn draw(mesh: &Mesh) -> Draw {
    Draw {
        vertices: mesh.vertices,
        indices: mesh.indices,
        first_index: 0,
        index_count: mesh.count,
    }
}
