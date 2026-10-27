use super::*;
impl GeometryView {
    /// Records both occlusion phases and material resolve. GPU-generated counts
    /// feed both raster backends without any host visibility readback.
    pub fn record(&mut self, gpu: &GpuDevice) -> Result<CommandList> {
        let mut g = Graph::default();
        let mut resources: Vec<_> = self.buffers.iter().map(|&b| Resource::Buffer(b)).collect();
        resources.push(Resource::Buffer(self.frame));
        resources.extend(self.images.map(Resource::Image));
        resources.push(Resource::Image(self.depth));
        resources.extend([
            Resource::Buffer(self.residency.table),
            Resource::Buffer(self.residency.readback),
            Resource::Buffer(self.residency.upload),
        ]);
        let ids: Vec<_> = resources
            .iter()
            .enumerate()
            .map(|(i, &r)| {
                g.import(
                    r,
                    if i == 13 || i == 21 {
                        State {
                            access: Access::HostWrite,
                            queue: Queue::Graphics,
                        }
                    } else {
                        self.previous.get(i).copied().unwrap_or(State {
                            access: if i < 13 || i == 19 {
                                Access::ShaderRead
                            } else {
                                Access::Undefined
                            },
                            queue: Queue::Graphics,
                        })
                    },
                )
            })
            .collect();
        let shared = [0, 1, 2, 3, 12, 13, 19];
        let texture_uses: Vec<_> = self
            .textures
            .into_iter()
            .flat_map(|b| b.resources())
            .map(|(resource, access)| {
                (
                    g.import(
                        resource,
                        State {
                            access,
                            queue: Queue::Graphics,
                        },
                    ),
                    access,
                )
            })
            .collect();
        let mut recipes = Vec::new();
        let mut add =
            |g: &mut Graph, name: &str, pipeline: &str, phase: u32, uses: &[(usize, Access)]| {
                let mut access: Vec<_> = shared
                    .iter()
                    .filter(|&&i| !uses.iter().any(|&(used, _)| used == i))
                    .map(|&i| (ids[i], Access::ShaderRead))
                    .collect();
                access.extend(uses.iter().map(|&(i, a)| (ids[i], a)));
                if pipeline == "resolve_textured" {
                    access.extend(texture_uses.iter().copied());
                }
                let id = g.pass(name, Queue::Graphics, access);
                recipes.push((id, pipeline.to_owned(), phase));
            };
        if self.residency.pending > 0 {
            add(
                &mut g,
                "Geo.StreamUpload",
                "stream_upload",
                0,
                &[
                    (3, Access::Storage),
                    (19, Access::Storage),
                    (21, Access::ShaderRead),
                ],
            );
        }
        add(
            &mut g,
            "Geo.Reset",
            "reset",
            0,
            &[
                (6, Access::Storage),
                (7, Access::Storage),
                (10, Access::Storage),
                (9, Access::Storage),
            ],
        );
        for phase in 0..2 {
            if phase == 1 {
                add(&mut g, "Geo.Requeue", "reset", 1, &[(6, Access::Storage)]);
            }
            add(
                &mut g,
                if phase == 0 {
                    "Geo.InstanceCull.Previous"
                } else {
                    "Geo.InstanceCull.Current"
                },
                "instances",
                phase,
                &[
                    (4, Access::Storage),
                    (6, Access::Storage),
                    (8, Access::ShaderRead),
                ],
            );
            add(
                &mut g,
                if phase == 0 {
                    "Geo.DagCut.Main"
                } else {
                    "Geo.DagCut.Post"
                },
                "select",
                phase,
                &[
                    (4, Access::ShaderRead),
                    (5, Access::Storage),
                    (6, Access::Storage),
                    (9, Access::Storage),
                ],
            );
            add(
                &mut g,
                if phase == 0 {
                    "Geo.Arguments.Main"
                } else {
                    "Geo.Arguments.Post"
                },
                "arguments",
                phase,
                &[(6, Access::Storage)],
            );
            add(
                &mut g,
                if phase == 0 {
                    "Geo.Software.Main"
                } else {
                    "Geo.Software.Post"
                },
                "software",
                phase,
                &[
                    (5, Access::ShaderRead),
                    (6, Access::Indirect),
                    (7, Access::Storage),
                    (9, Access::Storage),
                    (10, Access::Storage),
                ],
            );
            add(
                &mut g,
                if phase == 0 {
                    "Geo.Hardware.Main"
                } else {
                    "Geo.Hardware.Post"
                },
                "hardware",
                phase,
                &[
                    (5, Access::ShaderRead),
                    (6, Access::Indirect),
                    (7, Access::Storage),
                    (10, Access::Storage),
                    (18, Access::Depth),
                ],
            );
            add(
                &mut g,
                if phase == 0 {
                    "Geo.Hzb.Main"
                } else {
                    "Geo.Hzb.Post"
                },
                "hzb",
                phase,
                &[(7, Access::ShaderRead), (8, Access::Storage)],
            );
        }
        add(
            &mut g,
            "Geo.MaterialResolve",
            if self.textures.is_some() {
                "resolve_textured"
            } else {
                "resolve"
            },
            0,
            &[
                (5, Access::ShaderRead),
                (6, Access::ShaderRead),
                (7, Access::ShaderRead),
                (10, Access::ShaderRead),
                (11, Access::Storage),
                (14, Access::Storage),
                (15, Access::Storage),
                (16, Access::Storage),
                (17, Access::Storage),
            ],
        );
        g.pass(
            "Geo.FeedbackCopy",
            Queue::Graphics,
            [
                (ids[9], Access::CopySource),
                (ids[20], Access::CopyDestination),
            ],
        );
        g.pass(
            "Geo.FeedbackReady",
            Queue::Graphics,
            [(ids[20], Access::HostRead), (ids[9], Access::Storage)],
        );
        g.pass(
            "Geo.Publish",
            Queue::Graphics,
            [11, 14, 15, 16, 17].map(|i| (ids[i], Access::ShaderRead)),
        );
        let compiled = g.compile()?;
        self.previous = compiled.final_states;
        let mut r = gpu.begin(Queue::Graphics);
        for pass in compiled.passes {
            r.barriers(pass.before);
            let Some((_, pipeline, phase)) = recipes.iter().find(|(id, _, _)| *id == pass.id)
            else {
                if pass.name == "Geo.FeedbackCopy" {
                    r.push(Command::BeginPass(pass.name));
                    r.push(Command::CopyBuffer {
                        source: self.buffers[9],
                        destination: self.residency.readback,
                        size: (u64::from(self.cluster_count) + 8) * 4,
                    });
                    r.push(Command::EndPass);
                }
                r.barriers(pass.after);
                continue;
            };
            r.push(Command::BeginPass(pass.name));
            r.push(Command::Pipeline(self.pipelines[pipeline]));
            r.push(Command::Constants(RootConstants {
                words: [self.slots[13], *phase, 0, 0, 0, 0, 0, 0],
            }));
            match pipeline.as_str() {
                "stream_upload" => r.push(Command::Dispatch([self.residency.pending, 1, 1])),
                "reset" => r.push(Command::Dispatch(if *phase == 0 {
                    groups(
                        (u64::from(self.size[0]) * u64::from(self.size[1]))
                            .max(u64::from(self.cluster_count) + 8),
                    )
                } else {
                    [1, 1, 1]
                })),
                "instances" => r.push(Command::Dispatch(groups(u64::from(self.instance_count)))),
                "select" => r.push(Command::Dispatch(groups(
                    u64::from(self.instance_count) * u64::from(self.cluster_count),
                ))),
                "arguments" => r.push(Command::Dispatch([1, 1, 1])),
                "software" => r.push(Command::DispatchIndirect {
                    arguments: self.buffers[6],
                    offset: 16,
                }),
                "hardware" => {
                    r.push(Command::BeginRendering {
                        colors: Vec::new(),
                        depth: Some(Attachment {
                            image: self.depth,
                            clear: if *phase == 0 { Some([0.0; 4]) } else { None },
                        }),
                        size: self.size,
                    });
                    r.push(if self.mesh {
                        Command::DrawMeshTasksIndirect {
                            arguments: self.buffers[6],
                            offset: 32,
                        }
                    } else {
                        Command::DrawIndirect {
                            arguments: self.buffers[6],
                            offset: 0,
                        }
                    });
                    r.push(Command::EndRendering);
                }
                "hzb" => r.push(Command::Dispatch([
                    self.size[0].div_ceil(16),
                    self.size[1].div_ceil(16),
                    1,
                ])),
                "resolve" | "resolve_textured" => r.push(Command::Dispatch([
                    self.size[0].div_ceil(8),
                    self.size[1].div_ceil(8),
                    1,
                ])),
                _ => unreachable!("every prepared recipe has an encoder"),
            }
            r.push(Command::EndPass);
            r.barriers(pass.after);
        }
        self.residency.pending = 0;
        Ok(r.finish())
    }
}
fn groups(items: u64) -> [u32; 3] {
    let count = items.div_ceil(64);
    [count.min(65535) as u32, count.div_ceil(65535) as u32, 1]
}
