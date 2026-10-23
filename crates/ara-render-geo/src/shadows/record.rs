use super::*;
impl VirtualShadows {
    /// Records page feedback/cache allocation and only dirty geometry through the
    /// same compressed-cluster fetch as camera visibility. No page-count readback.
    pub fn record(
        &mut self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
    ) -> Result<CommandList> {
        let mut graph = Graph::default();
        let mut resources: Vec<_> = self.buffers.iter().map(|&b| Resource::Buffer(b)).collect();
        resources.push(Resource::Buffer(self.frame));
        resources.push(Resource::Image(self.mask));
        let ids: Vec<_> = resources
            .iter()
            .enumerate()
            .map(|(i, &r)| {
                graph.import(
                    r,
                    if i == 8 {
                        State {
                            access: Access::HostWrite,
                            queue: Queue::Graphics,
                        }
                    } else {
                        self.previous.get(i).copied().unwrap_or(State {
                            access: if i == 9 || i == 3 {
                                Access::Undefined
                            } else {
                                Access::ShaderRead
                            },
                            queue: Queue::Graphics,
                        })
                    },
                )
            })
            .collect();
        let world = graph.import(
            Resource::Image(geometry.images[2]),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let normal = graph.import(
            Resource::Image(geometry.images[1]),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let preview = graph.import(
            Resource::Image(geometry.images[3]),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let scene: Vec<_> = geometry.buffers[..4]
            .iter()
            .map(|&b| {
                (
                    graph.import(
                        Resource::Buffer(b),
                        State {
                            access: Access::ShaderRead,
                            queue: Queue::Graphics,
                        },
                    ),
                    Access::ShaderRead,
                )
            })
            .collect();
        let table = graph.import(
            Resource::Buffer(geometry.residency.table),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let mut recipes = Vec::new();
        for (name, uses) in [
            (
                "shadow_reset",
                vec![
                    (0, Access::Storage),
                    (1, Access::Storage),
                    (2, Access::Storage),
                    (4, Access::Storage),
                    (7, Access::Storage),
                ],
            ),
            ("shadow_mark", vec![(1, Access::Storage)]),
            (
                "shadow_allocate",
                vec![
                    (0, Access::Storage),
                    (1, Access::ShaderRead),
                    (2, Access::Storage),
                    (4, Access::Storage),
                    (5, Access::Storage),
                ],
            ),
            ("shadow_finalize", vec![(4, Access::Storage)]),
            (
                "shadow_clear",
                vec![
                    (2, Access::ShaderRead),
                    (3, Access::Storage),
                    (4, Access::Indirect),
                    (5, Access::ShaderRead),
                ],
            ),
            (
                "shadow_select",
                vec![
                    (0, Access::ShaderRead),
                    (2, Access::ShaderRead),
                    (4, Access::Indirect),
                    (6, Access::Storage),
                    (7, Access::Storage),
                ],
            ),
            ("shadow_arguments", vec![(7, Access::Storage)]),
            (
                "shadow_raster",
                vec![
                    (0, Access::ShaderRead),
                    (2, Access::ShaderRead),
                    (3, Access::Storage),
                    (6, Access::ShaderRead),
                    (7, Access::Indirect),
                ],
            ),
            (
                "shadow_resolve",
                vec![
                    (0, Access::ShaderRead),
                    (2, Access::ShaderRead),
                    (3, Access::ShaderRead),
                    (9, Access::Storage),
                ],
            ),
        ] {
            let mut uses: Vec<_> = uses.into_iter().map(|(i, a)| (ids[i], a)).collect();
            uses.push((ids[8], Access::ShaderRead));
            if name == "shadow_mark" || name == "shadow_resolve" {
                uses.extend([(world, Access::ShaderRead), (normal, Access::ShaderRead)]);
            }
            if name == "shadow_resolve" {
                uses.push((preview, Access::Storage));
            }
            if name == "shadow_select" || name == "shadow_raster" {
                uses.extend(scene.iter().copied());
                uses.push((table, Access::ShaderRead));
            }
            let id = graph.pass(format!("VSM.{name}"), Queue::Graphics, uses);
            recipes.push((id, name));
        }
        graph.pass(
            "VSM.Publish",
            Queue::Graphics,
            [(ids[9], Access::ShaderRead), (preview, Access::ShaderRead)],
        );
        let compiled = graph.compile()?;
        self.previous = compiled.final_states[..10].to_vec();
        let frame_slot = gpu.storage_buffer(self.frame)?.index;
        let mut r = gpu.begin(Queue::Graphics);
        for p in compiled.passes {
            r.barriers(p.before);
            let Some((_, name)) = recipes.iter().find(|(id, _)| *id == p.id) else {
                r.barriers(p.after);
                continue;
            };
            r.push(Command::BeginPass(p.name));
            r.push(Command::Pipeline(self.pipelines[*name]));
            r.push(Command::Constants(RootConstants {
                words: [frame_slot, 0, 0, 0, 0, 0, 0, 0],
            }));
            match *name {
                "shadow_reset" => r.push(Command::Dispatch([1536, 1, 1])),
                "shadow_mark" | "shadow_resolve" => r.push(Command::Dispatch([
                    self.size[0].div_ceil(8),
                    self.size[1].div_ceil(8),
                    1,
                ])),
                "shadow_allocate" => r.push(Command::Dispatch([
                    (self.projection_count as u32 * 16384).div_ceil(64),
                    1,
                    1,
                ])),
                "shadow_finalize" | "shadow_arguments" => r.push(Command::Dispatch([1, 1, 1])),
                "shadow_clear" => r.push(Command::DispatchIndirect {
                    arguments: self.buffers[4],
                    offset: 0,
                }),
                "shadow_select" => r.push(Command::DispatchIndirect {
                    arguments: self.buffers[4],
                    offset: 32,
                }),
                "shadow_raster" => {
                    r.push(Command::BeginRendering {
                        colors: Vec::new(),
                        depth: None,
                        size: [16384, 16384],
                    });
                    r.push(Command::DrawIndirect {
                        arguments: self.buffers[7],
                        offset: 0,
                    });
                    r.push(Command::EndRendering);
                }
                _ => unreachable!("prepared VSM recipe"),
            }
            r.push(Command::EndPass);
            r.barriers(p.after);
        }
        Ok(r.finish())
    }
}
