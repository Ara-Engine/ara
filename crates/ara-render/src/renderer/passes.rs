use super::*;
use crate::{
    capture::ViewMode,
    graph::{Graph, PassId},
    look::{
        frame::{FrameInputs, LookFrame},
        passes::LookPass,
    },
};
use ara_gpu::{
    RootConstants,
    vulkan::{Attachment, Command, Draw},
};

/// What each declared graph pass records.
/// Native draw list of the arbitrary-matrix shadow pass (`cull_native_shadow`).
const FAR_SHADOW_PASS: usize = 6;

#[derive(Clone, Copy)]
enum Recipe {
    Shadow,
    /// The far shadow map (`look::far`), only when its view changed.
    FarShadow,
    /// Local-light shadow faces (`look::local`) that must be redrawn.
    LocalShadow,
    GBuffer,
    Tile,
    Lighting,
    Forward,
    Taa,
    Tonemap,
    Look(LookPass),
    /// Ownership/synchronization only.
    Empty,
}

impl Renderer {
    pub(super) fn render_frame(
        &mut self,
        camera_spec: &CameraSpec,
        exposure: f32,
        deterministic: bool,
        mode: ViewMode,
        capture: bool,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Raster.render_frame");
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
        if let Some(native) = &mut self.native {
            native.prepare(&mut self.gpu, &frame, &camera)?;
        }
        self.gpu.upload(self.frame, bytemuck::bytes_of(&frame))?;
        // The far map redraws only when its view, content or casters inside it
        // changed; its draws read the far matrix as cascade 0 of their own frame
        // record, and native casters come from a culled pass of their own.
        // Cached shadow maps forget what changed casters reach; legacy (non-native)
        // content reports no changes, so its maps redraw every frame.
        if self.look.cascades.is_none() {
            self.ensure_cascades()?;
        }
        let (mut far, mut cascades, mut local) = (
            self.look.far.take(),
            self.look.cascades.take(),
            self.look.local.take(),
        );
        let (all, bounds) = match self.native_caster_changes() {
            Some(changes) => (changes.all, changes.bounds),
            None => (true, &[][..]),
        };
        if let Some(far) = &mut far {
            far.cache.observe(all, bounds);
        }
        if let Some(cascades) = &mut cascades {
            cascades.observe(all, bounds);
        }
        if let Some(local) = &mut local {
            local.cache.observe(all, bounds);
        }
        (self.look.far, self.look.cascades, self.look.local) = (far, cascades, local);
        let cascade_draw: Vec<usize> = self.look.cascades.as_ref().map_or_else(Vec::new, |c| {
            (0..c.images.len())
                .filter(|&i| c.needs_draw(i, frame.shadows[i]))
                .collect()
        });
        let far_draw = match &self.look.far {
            Some(far) => {
                let (matrix, view) = crate::look::far::FarMap::fit(
                    far.settings,
                    camera.position,
                    self.content.sun_direction,
                );
                far.cache.needs_draw(&view).then_some((matrix, view))
            }
            None => None,
        };
        if let (Some((matrix, _)), Some(far)) = (far_draw, &self.look.far) {
            let mut far_frame = frame;
            far_frame.shadows[0] = matrix;
            self.gpu.upload(far.frame, bytemuck::bytes_of(&far_frame))?;
            if self.native.is_some() {
                self.cull_native_shadow(matrix)?;
            }
        }
        // Local-light shadows (look::local): this frame's faces are culled as
        // native shadow passes 7+face and drawn as cascade 4+face, whose matrix
        // the table (rewritten every frame) holds.
        let mut local_draws: Vec<(usize, usize, glam::Mat4)> = Vec::new();
        if self.native.is_some()
            && let Some(mut local) = self.look.local.take()
        {
            let result = (|| -> Result<()> {
                self.reserve_native_shadow_passes(crate::look::local::capacity(local.settings))?;
                let plan = local.cache.plan(
                    &self.content.lights,
                    camera.projection * camera.view,
                    camera.position,
                    local.size,
                    capture || deterministic,
                );
                for &(face, matrix) in &plan.draws {
                    let pass = self.cull_native_shadow_pass(face, matrix)?;
                    local_draws.push((face, pass, matrix));
                }
                self.gpu.upload(local.table, &plan.table)?;
                Ok(())
            })();
            self.look.local = Some(local);
            result?;
        }
        let mut compute_frame = frame;
        compute_frame.lighting[3] = self.content.compute_light_index;
        self.gpu
            .upload(self.compute_frame, bytemuck::bytes_of(&compute_frame))?;
        self.look.ensure(&mut self.gpu)?;
        let (sampled, storage) = self.look.indices();
        let look_frame = LookFrame::new(
            &self.look.effective(),
            &FrameInputs {
                sun_lux: self.content.sun_lux,
                exposure,
                reset: self.targets.history.is_none() || self.look.settle > 0,
                frame_number: self.frame_number,
                sampled,
                storage,
                state: self.look.state_index,
            },
        );
        let mut look_frame = look_frame;
        if let Some(cascades) = &self.look.cascades {
            for (slot, &index) in look_frame.cascade_maps.iter_mut().zip(&cascades.sampled) {
                *slot = index;
            }
        }
        if let Some(far) = &self.look.far {
            let drawn = far_draw
                .map(|(matrix, _)| matrix)
                .or(far.cache.view.map(|_| far.cache.matrix));
            if let Some(matrix) = drawn {
                look_frame.far_shadow = matrix.to_cols_array_2d();
                look_frame.far_info = [far.sampled, far.settings.resolution, 1, 0];
            }
        }
        if let Some(gi) = &self.look.gi {
            gi.write(&mut look_frame, self.look.quality.gi_budget);
            look_frame.gi_gather = self.look.gather_indices();
        }
        look_frame.terrain[0] = self.look.quality.terrain_detail;
        if let Some(local) = &self.look.local
            && self.native.is_some()
        {
            look_frame.local_shadows = [
                local.table_index,
                local.sampled,
                local.size[0],
                local.size[1],
            ];
        }
        if let Some(r) = self.look.quality.reflections
            && self.targets.history.is_some()
        {
            look_frame.flags[1] |= crate::look::frame::REFLECTIONS;
            look_frame.ssr = [r.max_roughness, r.distance, r.steps as f32, 0.];
            look_frame.ssr_images = self.look.ssr_indices();
        }
        if let Some(ao) = self.look.quality.ambient_occlusion {
            look_frame.flags[1] |= crate::look::frame::AMBIENT_OCCLUSION;
            look_frame.ao = [ao.radius, ao.slices as f32, ao.steps as f32, 0.];
            look_frame.ao_images = self.look.ao_indices();
        }
        self.gpu
            .upload(self.look.frame, bytemuck::bytes_of(&look_frame))?;
        let mut graph = Graph::default();
        let mut recipes = Vec::new();
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
        let look = self.look.import(&mut graph);
        // Conservative bindless reachability of every graphics-queue shader pass.
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
        if let Some(native) = &self.native {
            native.resources(&mut |resource, access| {
                let id = graph.import(
                    resource,
                    State {
                        access,
                        queue: Queue::Graphics,
                    },
                );
                graphics.push((id, Access::ShaderRead));
            });
        }
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
        let mut declare = |id: PassId, recipe: Recipe| {
            debug_assert_eq!(id.0, recipes.len(), "passes are declared in order");
            recipes.push(recipe);
        };
        // Only cascades whose matrix or casters changed are redrawn; shadow passes
        // read materials (alpha tests) like every shaded pass.
        let mut shadow_passes = Vec::new();
        if !cascade_draw.is_empty() {
            let shadow = graph.pass(
                "ShadowAtlas.Cascades",
                Queue::Graphics,
                cascade_draw
                    .iter()
                    .map(|&i| (look.cascades[i], Access::Depth)),
            );
            declare(shadow, Recipe::Shadow);
            shadow_passes.push(shadow);
        }
        let cascade_reads: Vec<_> = look
            .cascades
            .iter()
            .map(|&id| (id, Access::ShaderRead))
            .collect();
        if let (Some(far), Some(_)) = (look.far, far_draw) {
            let pass = graph.pass("ShadowAtlas.Far", Queue::Graphics, [(far, Access::Depth)]);
            declare(pass, Recipe::FarShadow);
            shadow_passes.push(pass);
        }
        let far_read = look.far.map(|far| (far, Access::ShaderRead));
        if let (Some(local), false) = (look.local, local_draws.is_empty()) {
            let pass = graph.pass(
                "ShadowAtlas.Local",
                Queue::Graphics,
                [(local, Access::Depth)],
            );
            declare(pass, Recipe::LocalShadow);
            shadow_passes.push(pass);
        }
        // Read only once the atlas holds a face (the table names no other).
        let local_written = !local_draws.is_empty()
            || self
                .look
                .local
                .as_ref()
                .is_some_and(|l| l.state.access != Access::Undefined);
        let local_read = look
            .local
            .filter(|_| local_written)
            .map(|local| (local, Access::ShaderRead));
        let gbuffer = graph.pass(
            "GBuffer",
            Queue::Graphics,
            [
                (ids[ALBEDO], Access::Color),
                (ids[NORMAL], Access::Color),
                (ids[MATERIAL], Access::Color),
                (ids[EMISSIVE], Access::Color),
                (ids[ID], Access::Color),
                (ids[DEPTH], Access::Depth),
            ]
            .into_iter()
            .chain(self.native.as_ref().map(|_| (ids[MOTION], Access::Color))),
        );
        declare(gbuffer, Recipe::GBuffer);
        let mut ao_reads = graphics.clone();
        ao_reads.extend([
            (ids[DEPTH], Access::ShaderRead),
            (ids[NORMAL], Access::ShaderRead),
        ]);
        for (id, pass) in self.look.ao_passes(&mut graph, &look, &ao_reads) {
            declare(id, Recipe::Look(pass));
        }
        let mut ssr_reads = ao_reads.clone();
        ssr_reads.push((ids[MATERIAL], Access::ShaderRead));
        let history = self.targets.history.map(|h| ids[h]);
        for (id, pass) in self.look.ssr_passes(&mut graph, &look, history, &ssr_reads) {
            declare(id, Recipe::Look(pass));
        }
        let mut sky_shared = graphics.clone();
        sky_shared.extend(cascade_reads.iter().copied());
        sky_shared.extend(far_read);
        let sun = {
            let d = self.content.sun_direction;
            let l = self.content.sun_lux;
            [d.x, d.y, d.z, l.x, l.y, l.z]
        };
        for (id, pass) in self.look.sky_passes(&mut graph, &look, &sky_shared, sun) {
            declare(id, Recipe::Look(pass));
        }
        let mut gather_reads = sky_shared.clone();
        gather_reads.extend([
            (ids[DEPTH], Access::ShaderRead),
            (ids[NORMAL], Access::ShaderRead),
        ]);
        for (id, pass) in self.look.gather_passes(&mut graph, &look, &gather_reads) {
            declare(id, Recipe::Look(pass));
        }
        let culling = graph.pass("TiledLightCull", Queue::Compute, [(tile, Access::Storage)]);
        declare(culling, Recipe::Tile);
        let reflected = self.look.quality.reflections.is_some() && self.targets.history.is_some();
        let look_reads = self.look.lighting_reads(&look, reflected);
        let lighting = graph.pass(
            "DeferredLighting",
            Queue::Graphics,
            [
                (ids[ALBEDO], Access::ShaderRead),
                (ids[NORMAL], Access::ShaderRead),
                (ids[MATERIAL], Access::ShaderRead),
                (ids[EMISSIVE], Access::ShaderRead),
                (ids[DEPTH], Access::ShaderRead),
                (tile, Access::ShaderRead),
                (ids[HDR], Access::Color),
            ]
            .into_iter()
            .chain(cascade_reads.iter().copied())
            .chain(far_read)
            .chain(local_read)
            .chain(look_reads.iter().copied()),
        );
        declare(lighting, Recipe::Lighting);
        let forward = graph.pass(
            "ForwardTransparency",
            Queue::Graphics,
            [
                (ids[HDR], Access::Color),
                (ids[ALBEDO], Access::Color),
                (ids[NORMAL], Access::Color),
                (ids[MATERIAL], Access::Color),
                (ids[EMISSIVE], Access::Color),
                (ids[DEPTH], Access::Depth),
                (ids[ID], Access::Color),
                (tile, Access::ShaderRead),
            ]
            .into_iter()
            .chain(cascade_reads.iter().copied())
            .chain(far_read)
            .chain(local_read)
            .chain(look_reads.iter().copied())
            .chain(self.native.as_ref().map(|_| (ids[MOTION], Access::Color))),
        );
        declare(forward, Recipe::Forward);
        let mut shaded = shadow_passes;
        shaded.extend([gbuffer, lighting, forward]);
        // Deferred post leaves the lit scene in HDR for external composites (`finish`).
        // Captures always produce their own output, never a deferred frame.
        let deferred = self.look.deferred && matches!(mode, ViewMode::Lit) && !capture;
        if !deferred {
            let mut taa = vec![
                (ids[HDR], Access::ShaderRead),
                (ids[DEPTH], Access::ShaderRead),
                (ids[output_history], Access::Storage),
                // History is stored pre-exposed (look.slangh exposureState).
                (look.state, Access::ShaderRead),
            ];
            if let Some(history) = self.targets.history {
                taa.push((ids[history], Access::ShaderRead));
            }
            if self.native.is_some() {
                taa.push((ids[MOTION], Access::ShaderRead));
            }
            let temporal = graph.pass("TemporalResolve", Queue::Graphics, taa);
            declare(temporal, Recipe::Taa);
            shaded.push(temporal);
        }
        if matches!(mode, ViewMode::Lit) && !deferred {
            for (id, pass) in self.look.post_passes(
                &mut graph,
                &look,
                ids[output_history],
                ids[OUTPUT],
                &graphics,
            ) {
                declare(id, Recipe::Look(pass));
            }
        } else if !matches!(mode, ViewMode::Lit) {
            let tonemap = graph.pass(
                "DebugView.SDR",
                Queue::Graphics,
                [
                    (ids[output_history], Access::ShaderRead),
                    (ids[OUTPUT], Access::Storage),
                    (ids[ALBEDO], Access::ShaderRead),
                    (ids[NORMAL], Access::ShaderRead),
                    (ids[MATERIAL], Access::ShaderRead),
                    (ids[EMISSIVE], Access::ShaderRead),
                    (ids[ID], Access::ShaderRead),
                    (ids[DEPTH], Access::ShaderRead),
                ]
                .into_iter()
                .chain(cascade_reads.iter().copied())
                .chain(far_read)
                .chain(local_read),
            );
            declare(tonemap, Recipe::Tonemap);
            shaded.push(tonemap);
        }
        // A deferred frame has no output yet; `finish` produces it.
        if !deferred {
            let ready = graph.pass(
                "PresentationReady",
                Queue::Graphics,
                [
                    (ids[OUTPUT], Access::CopySource),
                    (ids[ID], Access::CopySource),
                ],
            );
            declare(ready, Recipe::Empty);
        } else {
            // External VFX can advance repeatedly before drawing. Publish sampled
            // inputs once so their declared states stay valid across those calls
            // and `finish` imports the same states the feature graph leaves behind.
            let ready = graph.pass(
                "FeatureInputsReady",
                Queue::Graphics,
                [
                    (ids[HDR], Access::ShaderRead),
                    (ids[DEPTH], Access::ShaderRead),
                    (ids[ID], Access::CopySource),
                ],
            );
            declare(ready, Recipe::Empty);
        }
        let ownership = graph.pass(
            "TileOwnershipReturn",
            Queue::Compute,
            [(tile, Access::Storage)],
        );
        declare(ownership, Recipe::Empty);
        for pass in shaded {
            graph.accesses(pass, graphics.iter().copied())?;
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
        graph.accesses(culling, compute)?;
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
        for batch in submission::batches(&compiled.passes) {
            let mut recorder = self.gpu.begin(batch[0].queue);
            for pass in batch {
                recorder.push(Command::BeginPass(pass.name.clone()));
                recorder.barriers(pass.before.iter().copied());
                match recipes[pass.id.0] {
                    Recipe::Shadow => {
                        let maps = self
                            .look
                            .cascades
                            .as_ref()
                            .expect("a cascade pass is declared only with cascade maps");
                        recorder.push(Command::Pipeline(self.pipelines.shadow));
                        for &index in &cascade_draw {
                            let cascade = index as u32;
                            recorder.push(Command::BeginRendering {
                                colors: Vec::new(),
                                depth: Some(Attachment {
                                    image: maps.images[index],
                                    clear: Some([0.; 4]),
                                }),
                                size: [maps.resolution; 2],
                            });
                            recorder.push(Command::Viewport {
                                origin: [0, 0],
                                size: [maps.resolution; 2],
                            });
                            if let Some(native) = &self.native {
                                let mut constants = root;
                                constants.words[4] = cascade;
                                native.record(&mut recorder, constants, cascade as usize);
                            }
                            for (index, mesh) in self
                                .content
                                .meshes
                                .iter()
                                .enumerate()
                                .filter(|(_, m)| self.native.is_none() && !m.transparent)
                            {
                                let mut constants = root;
                                constants.words[2] = mesh.material;
                                constants.words[3] = index as u32 + 1;
                                constants.words[4] = cascade;
                                recorder.push(Command::Constants(constants));
                                recorder.push(Command::Draw(draw(mesh)));
                            }
                            recorder.push(Command::EndRendering);
                        }
                    }
                    Recipe::FarShadow => {
                        let far = self
                            .look
                            .far
                            .as_ref()
                            .expect("a far pass is declared only with a far map");
                        recorder.push(Command::Pipeline(self.pipelines.shadow));
                        recorder.push(Command::BeginRendering {
                            colors: Vec::new(),
                            depth: Some(Attachment {
                                image: far.image,
                                clear: Some([0.; 4]),
                            }),
                            size: [far.settings.resolution; 2],
                        });
                        recorder.push(Command::Viewport {
                            origin: [0, 0],
                            size: [far.settings.resolution; 2],
                        });
                        let mut constants = root;
                        constants.words[0] = far.frame_index;
                        constants.words[4] = 0;
                        if let Some(native) = &self.native {
                            native.record(&mut recorder, constants, FAR_SHADOW_PASS);
                        }
                        for (index, mesh) in self
                            .content
                            .meshes
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| self.native.is_none() && !m.transparent)
                        {
                            constants.words[2] = mesh.material;
                            constants.words[3] = index as u32 + 1;
                            recorder.push(Command::Constants(constants));
                            recorder.push(Command::Draw(draw(mesh)));
                        }
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::LocalShadow => {
                        let local = self
                            .look
                            .local
                            .as_ref()
                            .expect("a local shadow pass is declared only with a local map");
                        let tile = local.settings.resolution;
                        recorder.push(Command::Pipeline(self.pipelines.shadow));
                        recorder.push(Command::BeginRendering {
                            colors: Vec::new(),
                            depth: Some(Attachment {
                                image: local.atlas,
                                clear: None,
                            }),
                            size: local.size,
                        });
                        for &(face, pass, _) in &local_draws {
                            let origin = crate::look::local::origin(face, tile);
                            recorder.push(Command::Viewport {
                                origin,
                                size: [tile; 2],
                            });
                            recorder.push(Command::ClearDepth {
                                origin,
                                size: [tile; 2],
                                depth: 0.,
                            });
                            let mut constants = root;
                            constants.words[4] = 4 + face as u32;
                            if let Some(native) = &self.native {
                                native.record(&mut recorder, constants, pass);
                            }
                        }
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::GBuffer => {
                        recorder.push(Command::Pipeline(self.pipelines.gbuffer));
                        recorder.push(Command::BeginRendering {
                            colors: [ALBEDO, NORMAL, MATERIAL, EMISSIVE, ID]
                                .into_iter()
                                .chain(self.native.as_ref().map(|_| MOTION))
                                .map(|i| Attachment {
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
                        if let Some(native) = &self.native {
                            native.record(&mut recorder, root, 4);
                        }
                        for (index, mesh) in self
                            .content
                            .meshes
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| self.native.is_none() && !m.transparent)
                        {
                            let mut constants = root;
                            constants.words[2] = mesh.material;
                            constants.words[3] = index as u32 + 1;
                            recorder.push(Command::Constants(constants));
                            recorder.push(Command::Draw(draw(mesh)));
                        }
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::Tile => {
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
                    Recipe::Lighting => {
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
                    Recipe::Forward => {
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
                                    image: self.targets.images[EMISSIVE],
                                    clear: None,
                                },
                            ]
                            .into_iter()
                            .chain(self.native.as_ref().map(|_| Attachment {
                                image: self.targets.images[MOTION],
                                clear: None,
                            }))
                            .collect(),
                            depth: Some(Attachment {
                                image: self.targets.images[DEPTH],
                                clear: None,
                            }),
                            size: self.size,
                        });
                        if let Some(native) = &self.native {
                            native.record(&mut recorder, root, 5);
                        }
                        let mut transparent = self
                            .content
                            .meshes
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| self.native.is_none() && m.transparent)
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
                    Recipe::Taa | Recipe::Tonemap => {
                        let taa = matches!(recipes[pass.id.0], Recipe::Taa);
                        recorder.push(Command::Pipeline(if taa {
                            self.pipelines.taa
                        } else {
                            self.pipelines.tonemap
                        }));
                        let mut constants = root;
                        constants.words[1] = if taa {
                            self.targets.sampled[HDR]
                        } else {
                            self.targets.sampled[output_history]
                        };
                        recorder.push(Command::Constants(constants));
                        recorder.push(Command::Dispatch([
                            self.size[0].div_ceil(8),
                            self.size[1].div_ceil(8),
                            1,
                        ]));
                    }
                    Recipe::Look(look_pass) => self.look.record(
                        &mut recorder,
                        look_pass,
                        self.frame_index,
                        // The reflection pyramid reads last frame's history; post passes
                        // read this frame's.
                        match look_pass {
                            LookPass::SsrPyramid(_) => {
                                self.targets.history.map_or(0, |h| self.targets.sampled[h])
                            }
                            _ => self.targets.sampled[output_history],
                        },
                        true,
                        self.size,
                    ),
                    Recipe::Empty => {}
                }
                recorder.barriers(pass.after.iter().copied());
                recorder.push(Command::EndPass);
            }
            let waits = submission::waits(batch, &points);
            let point = self.gpu.submit(recorder.finish(), &waits)?;
            points.resize(points.len() + batch.len(), point);
        }
        self.last_timings.clear();
        points.dedup();
        for point in points {
            self.last_timings.extend(self.gpu.resolve(point)?);
        }
        for (i, id) in ids.iter().enumerate() {
            self.targets.states[i] = compiled.final_states[id.0];
        }
        self.look.finish(&look, &compiled.final_states);
        if let (Some(far), Some((matrix, view))) = (&mut self.look.far, far_draw) {
            far.cache.drawn(matrix, view);
        }
        if let Some(local) = &mut self.look.local {
            for &(face, _, matrix) in &local_draws {
                local.cache.drawn(face, matrix);
            }
        }
        if let Some(cascades) = &mut self.look.cascades {
            for &index in &cascade_draw {
                cascades.drawn(index, frame.shadows[index]);
            }
        }
        if let Some(gi) = &mut self.look.gi {
            gi.advance(self.look.quality.gi_budget);
        }
        self.look.settle = self.look.settle.saturating_sub(1);
        // A deferred frame keeps the previous history for `finish` to resolve against.
        if deferred {
            self.look.deferred_history = Some(output_history);
        } else {
            self.look.deferred_history = None;
            self.targets.history = Some(output_history);
        }
        self.previous_vp = frame.vp;
        self.frame_number = self.frame_number.wrapping_add(1);
        self.gpu.assert_clean()?;
        if let Some(native) = &mut self.native {
            native.presented(&camera);
        }
        Ok(())
    }
}
impl Renderer {
    /// Leave Lit frames in linear HDR (`feature_color`) for external composites
    /// (landscape, particles), then grade them with [`Self::finish`].
    pub fn set_deferred_post(&mut self, deferred: bool) {
        self.look.deferred = deferred;
    }

    /// Temporal resolve, exposure, bloom and display grading of a composited frame:
    /// `color` is linear HDR at the render size, owned by Graphics in `state`, and
    /// stays readable. After a deferred-post `render` it resolves into that frame's
    /// history slot (antialiasing particles and landscape with the scene); otherwise
    /// it grades `color` directly. Reuses the frame's constants; returns the SDR
    /// output in CopySource.
    pub fn finish(&mut self, color: Image, sampled: u32, state: State) -> Result<Image> {
        self.look.ensure(&mut self.gpu)?;
        let mut graph = Graph::default();
        let input = graph.import(Resource::Image(color), state);
        let output = graph.import(
            Resource::Image(self.targets.images[OUTPUT]),
            self.targets.states[OUTPUT],
        );
        let slot = self.look.deferred_history.take();
        let mut imported = Vec::new();
        let mut import = |graph: &mut Graph, target: usize| {
            let id = graph.import(
                Resource::Image(self.targets.images[target]),
                self.targets.states[target],
            );
            imported.push((target, id));
            id
        };
        let resolve = slot.map(|slot| {
            let mut uses = vec![
                (input, Access::ShaderRead),
                (import(&mut graph, DEPTH), Access::ShaderRead),
                (import(&mut graph, slot), Access::Storage),
            ];
            if let Some(history) = self.targets.history {
                uses.push((import(&mut graph, history), Access::ShaderRead));
            }
            (slot, uses)
        });
        let look = self.look.import(&mut graph);
        let frame = graph.import(
            Resource::Buffer(self.frame),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        let resolved = resolve.map(|(slot, uses)| {
            let id = graph.pass(
                "TemporalResolve",
                Queue::Graphics,
                uses.into_iter().chain([
                    (frame, Access::ShaderRead),
                    (look.state, Access::ShaderRead),
                ]),
            );
            let slot_id = imported
                .iter()
                .find(|(target, _)| *target == slot)
                .map(|(_, id)| *id)
                .expect("the resolve imported its history slot");
            (id, slot, slot_id)
        });
        let (post_input, post_sampled) = match resolved {
            Some((_, slot, slot_id)) => (slot_id, self.targets.sampled[slot]),
            None => (input, sampled),
        };
        let passes = self.look.post_passes(
            &mut graph,
            &look,
            post_input,
            output,
            &[(frame, Access::ShaderRead)],
        );
        graph.pass(
            "Output.Ready",
            Queue::Graphics,
            [(input, Access::ShaderRead), (output, Access::CopySource)],
        );
        let compiled = graph.compile()?;
        let mut points = Vec::new();
        for batch in submission::batches(&compiled.passes) {
            let mut recorder = self.gpu.begin(batch[0].queue);
            for pass in batch {
                recorder.push(Command::BeginPass(pass.name.clone()));
                recorder.barriers(pass.before.iter().copied());
                if let Some((_, look_pass)) = passes.iter().find(|(id, _)| *id == pass.id) {
                    self.look.record(
                        &mut recorder,
                        *look_pass,
                        self.frame_index,
                        post_sampled,
                        resolved.is_some(),
                        self.size,
                    );
                } else if resolved.is_some_and(|(id, _, _)| id == pass.id) {
                    recorder.push(Command::Pipeline(self.pipelines.taa));
                    recorder.push(Command::Constants(RootConstants {
                        words: [self.frame_index, sampled, 0, 0, 0, 0, 0, 0],
                    }));
                    recorder.push(Command::Dispatch([
                        self.size[0].div_ceil(8),
                        self.size[1].div_ceil(8),
                        1,
                    ]));
                }
                recorder.barriers(pass.after.iter().copied());
                recorder.push(Command::EndPass);
            }
            let waits = submission::waits(batch, &points);
            let point = self.gpu.submit(recorder.finish(), &waits)?;
            points.resize(points.len() + batch.len(), point);
        }
        points.dedup();
        for point in points {
            self.last_timings.extend(self.gpu.resolve(point)?);
        }
        self.targets.states[OUTPUT] = compiled.final_states[output.0];
        for (target, id) in imported {
            self.targets.states[target] = compiled.final_states[id.0];
        }
        if let Some((_, slot, _)) = resolved {
            self.targets.history = Some(slot);
        }
        self.look.finish(&look, &compiled.final_states);
        self.gpu.assert_clean()?;
        Ok(self.targets.images[OUTPUT])
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
