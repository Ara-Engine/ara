use super::*;
impl Landscape {
    /// Records graph passes with complete bindless reachability. The host submits
    /// asynchronously; diagnostic examples explicitly resolve for timestamps/readback.
    pub fn record(&mut self, gpu: &VulkanDevice) -> Result<CommandList> {
        let mut g = Graph::default();
        let mut i = 0;
        let mut import = |resource, initial| {
            let state = if resource == Resource::Buffer(self.frame)
                || resource == Resource::Buffer(self.upload)
            {
                State {
                    access: Access::HostWrite,
                    queue: Queue::Graphics,
                }
            } else {
                self.previous.get(i).copied().unwrap_or(State {
                    access: initial,
                    queue: Queue::Graphics,
                })
            };
            i += 1;
            g.import(resource, state)
        };
        let resources = [
            self.frame,
            self.samples,
            self.instances,
            self.levels,
            self.lists,
            self.args,
            self.visible,
            self.foliage_args,
            self.parameters,
        ]
        .map(|b| {
            import(
                Resource::Buffer(b),
                if b == self.frame || b == self.parameters {
                    Access::HostWrite
                } else {
                    Access::ShaderRead
                },
            )
        });
        let color = import(Resource::Image(self.color), Access::Undefined);
        let depth = import(Resource::Image(self.depth), Access::Undefined);
        let shadow = import(Resource::Image(self.shadow), Access::Undefined);
        let hzb = import(Resource::Image(self.hzb), Access::Undefined);
        let upload = import(Resource::Buffer(self.upload), Access::HostWrite);
        let tex: Vec<_> = self
            .textures
            .iter()
            .map(|image| import(Resource::Image(*image), Access::ShaderRead))
            .collect();
        let geometry = self
            .geometry
            .map(|b| import(Resource::Buffer(b), Access::ShaderRead));
        let [
            frame,
            samples,
            instances,
            levels,
            lists,
            args,
            visible,
            fargs,
            parameters,
        ] = resources;
        let add = |g: &mut Graph, name: &str, mut uses: Vec<(ResourceId, Access)>| {
            if let Some(geometry) = geometry {
                uses.push((geometry, Access::ShaderRead));
                if matches!(name, "foliage_shadow" | "foliage_depth" | "foliage_color") {
                    uses.extend(tex.iter().map(|id| (*id, Access::ShaderRead)));
                }
            }
            g.pass(name, Queue::Graphics, uses)
        };
        if self.pending_uploads > 0 {
            add(
                &mut g,
                "tile_upload",
                vec![
                    (frame, Access::ShaderRead),
                    (upload, Access::ShaderRead),
                    (samples, Access::Storage),
                ],
            );
        }
        add(
            &mut g,
            "reset",
            vec![
                (frame, Access::ShaderRead),
                (args, Access::Storage),
                (fargs, Access::Storage),
            ],
        );
        add(
            &mut g,
            "select",
            vec![
                (frame, Access::ShaderRead),
                (samples, Access::ShaderRead),
                (levels, Access::Storage),
            ],
        );
        add(
            &mut g,
            "terrain_cull",
            vec![
                (frame, Access::ShaderRead),
                (samples, Access::ShaderRead),
                (levels, Access::ShaderRead),
                (lists, Access::Storage),
                (args, Access::Storage),
            ],
        );
        for (name, target) in [("terrain_shadow", shadow), ("terrain_depth", depth)] {
            add(
                &mut g,
                name,
                vec![
                    (frame, Access::ShaderRead),
                    (samples, Access::ShaderRead),
                    (levels, Access::ShaderRead),
                    (lists, Access::ShaderRead),
                    (args, Access::Indirect),
                    (target, Access::Depth),
                ],
            );
        }
        add(
            &mut g,
            "hzb",
            vec![
                (frame, Access::ShaderRead),
                (depth, Access::ShaderRead),
                (hzb, Access::Storage),
            ],
        );
        add(
            &mut g,
            "foliage_cull",
            vec![
                (frame, Access::ShaderRead),
                (instances, Access::ShaderRead),
                (visible, Access::Storage),
                (fargs, Access::Storage),
                (hzb, Access::ShaderRead),
            ],
        );
        for (name, target) in [("foliage_shadow", shadow), ("foliage_depth", depth)] {
            add(
                &mut g,
                name,
                vec![
                    (frame, Access::ShaderRead),
                    (instances, Access::ShaderRead),
                    (visible, Access::ShaderRead),
                    (fargs, Access::Indirect),
                    (target, Access::Depth),
                ],
            );
        }
        add(
            &mut g,
            "sky",
            vec![(frame, Access::ShaderRead), (color, Access::Color)],
        );
        let mut uses = vec![
            (frame, Access::ShaderRead),
            (samples, Access::ShaderRead),
            (levels, Access::ShaderRead),
            (lists, Access::ShaderRead),
            (args, Access::Indirect),
            (parameters, Access::ShaderRead),
            (depth, Access::Depth),
            (shadow, Access::ShaderRead),
            (color, Access::Color),
        ];
        uses.extend(tex.iter().map(|id| (*id, Access::ShaderRead)));
        add(&mut g, "terrain_color", uses);
        add(
            &mut g,
            "foliage_color",
            vec![
                (frame, Access::ShaderRead),
                (samples, Access::ShaderRead),
                (instances, Access::ShaderRead),
                (visible, Access::ShaderRead),
                (fargs, Access::Indirect),
                (depth, Access::Depth),
                (shadow, Access::ShaderRead),
                (color, Access::Color),
            ],
        );
        let compiled = g.compile()?;
        self.previous = compiled.final_states;
        // update() finishes before submission; declare its actual host write each frame.
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([ara_gpu::Barrier {
            resource: Resource::Buffer(self.frame),
            before: Access::HostWrite,
            after: Access::ShaderRead,
            handoff: None,
        }]);
        for pass in compiled.passes {
            r.barriers(pass.before);
            r.push(Command::BeginPass(pass.name.to_owned()));
            r.push(Command::Pipeline(self.pipelines[pass.name.as_str()]));
            let mut constants = RootConstants {
                words: [
                    self.slots[0],
                    0,
                    0,
                    self.image_slots[0],
                    self.image_slots[1],
                    self.image_slots[2],
                    self.image_slots[3],
                    self.upload_slot,
                ],
            };
            match pass.name.as_str() {
                "reset" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::Dispatch([1, 1, 1]));
                }
                "tile_upload" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::Dispatch([
                        (33u32 * 33 * self.pending_uploads).div_ceil(64),
                        1,
                        1,
                    ]));
                }
                "select" | "terrain_cull" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::Dispatch([
                        (self.tiles * self.tiles).div_ceil(64),
                        1,
                        1,
                    ]));
                }
                "hzb" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::Dispatch([
                        self.size[0].div_ceil(32),
                        self.size[1].div_ceil(32),
                        1,
                    ]));
                }
                "foliage_cull" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::Dispatch([self.count.div_ceil(64), 1, 1]));
                }
                "sky" => {
                    r.push(Command::Constants(constants));
                    r.push(Command::BeginRendering {
                        colors: vec![Attachment {
                            image: self.color,
                            clear: Some([0., 0., 0., 1.]),
                        }],
                        depth: None,
                        size: self.size,
                    });
                    r.push(Command::Fullscreen);
                    r.push(Command::EndRendering);
                }
                name => {
                    let shadow_pass = name.ends_with("shadow");
                    let color_pass = name.ends_with("color");
                    let foliage = name.starts_with("foliage");
                    constants.words[2] = u32::from(shadow_pass);
                    r.push(Command::BeginRendering {
                        colors: if color_pass {
                            vec![Attachment {
                                image: self.color,
                                clear: None,
                            }]
                        } else {
                            vec![]
                        },
                        depth: Some(Attachment {
                            image: if shadow_pass { self.shadow } else { self.depth },
                            clear: if name == "terrain_depth" || name == "terrain_shadow" {
                                Some([0.; 4])
                            } else {
                                None
                            },
                        }),
                        size: if shadow_pass { [2048, 2048] } else { self.size },
                    });
                    for group in 0..if foliage { 9 } else { 6 } {
                        constants.words[1] = group;
                        r.push(Command::Constants(constants));
                        r.push(Command::DrawIndirect {
                            arguments: if foliage {
                                self.foliage_args
                            } else {
                                self.args
                            },
                            offset: u64::from(group) * 16,
                        });
                    }
                    r.push(Command::EndRendering);
                }
            }
            r.push(Command::EndPass);
            r.barriers(pass.after);
        }
        self.pending_uploads = 0;
        Ok(r.finish())
    }
}
