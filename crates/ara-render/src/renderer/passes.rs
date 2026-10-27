use super::*;
use crate::{
    capture::ViewMode,
    graph::{Graph, PassId},
    look::{
        frame::{FrameInputs, LookFrame},
        gpu::DOF_COMPOSITE,
        passes::{Frames, LookPass},
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
    /// The horizon's heightfield capture (`look::horizon`) from native pass `.0`.
    HorizonCapture(usize),
    /// This frame's virtual shadow pages (`look::vsm`).
    VsmPages,
    GBuffer,
    Tile,
    Lighting,
    /// Blended and transmissive draws; `true` through the native lit PSO, which
    /// leaves the G-buffer alone (only the debug views read it afterwards).
    Forward(bool),
    Taa,
    /// One Ara TSR dispatch (`crate::tsr`), declared instead of `Taa` when selected.
    Tsr(crate::tsr::TsrPass),
    Tonemap,
    /// The camera's native draws again into the wire mask (`ViewMode::wire`).
    Wire,
    /// The wire mask over the display output; `true` over the lit image (one
    /// colour), `false` over the wireframe view's background (per-entity colours).
    WireComposite(bool),
    Look(LookPass),
    /// A hybrid ray-query pass (`renderer::rt`).
    Rays(super::rt::RtPass),
    /// The refraction copy (0) or a blurred level (1..4) of `look::refract`.
    Refract(u32),
    /// Blended casters' depth and film opacity from the sun (`look::translucent`).
    TranslucentSun,
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
        self.collect_view_resources()?;
        // Reflection captures bake off the hot path (look::captures; never per
        // frame), before this frame reads them.
        self.bake_reflection_captures(deterministic || capture);
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
        let mut frame = self.frame_data(&camera, exposure, deterministic, mode, output_history);
        if let Some(native) = &mut self.native {
            native.prepare(&mut self.gpu, &frame, &camera, &self.look.quality)?;
        }
        self.rays.prepare(
            &mut self.gpu,
            self.native.as_ref(),
            &self.look.quality,
            self.render,
            !self.secondary_active,
            deterministic,
            &self.artifacts,
            &self.shader_metadata.content_key,
        )?;
        // Transmissive forward draws in view refract a copy of the lit scene
        // (look::refract); other frames declare and allocate nothing for it.
        let refract_rect = self
            .native
            .as_ref()
            .and_then(|native| native.refraction_rect(self.render));
        // The far map redraws only when its view, content or casters inside it
        // changed; its draws read the far matrix as cascade 0 of their own frame
        // record, and native casters come from a culled pass of their own.
        // Cached shadow maps forget what changed casters reach; legacy (non-native)
        // content reports no changes, so its maps redraw every frame.
        if self.look.cascades.is_none() {
            self.ensure_cascades()?;
        }
        let (mut far, mut cascades, mut local, mut translucent) = (
            self.look.far.take(),
            self.look.cascades.take(),
            self.look.local.take(),
            self.look.translucent.take(),
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
        if let Some(translucent) = &mut translucent {
            translucent.observe(all, bounds);
        }
        (
            self.look.far,
            self.look.cascades,
            self.look.local,
            self.look.translucent,
        ) = (far, cascades, local, translucent);
        if all && let Some(horizon) = &mut self.look.horizon {
            horizon.invalidate();
        }
        // Cascades that wait for their redraw keep their drawn matrices in the
        // frame record, so the record is uploaded after this plan.
        let cascade_draw = match &mut self.look.cascades {
            Some(cascades) => cascades.plan(
                &mut frame.shadows,
                &frame::cascade_slices(&camera, frame.splits),
            ),
            None => Vec::new(),
        };
        self.gpu
            .upload_queued(self.frame, bytemuck::bytes_of(&frame))?;
        let mut display_frame = frame;
        display_frame.extent[..2].copy_from_slice(&self.size);
        self.gpu
            .upload_queued(self.display_frame, bytemuck::bytes_of(&display_frame))?;
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
            self.gpu
                .upload_queued(far.frame, bytemuck::bytes_of(&far_frame))?;
            if self.native.is_some() {
                self.cull_native_shadow(matrix)?;
            }
        }
        // Shadow patches (look::cascades, look::local) cull the casters under them
        // into native shadow slots after the local faces': one per cascade, then
        // `local::PATCHES`. Cascades draw them through their own frame record, whose
        // matrix is cropped to the patch. Legacy content reports no caster changes,
        // so it never patches.
        let local_capacity = self
            .look
            .local
            .as_ref()
            .map_or(0, |local| crate::look::local::capacity(local.settings));
        let patch_slots = local_capacity + 4 + crate::look::local::PATCHES;
        // Content and views are created with rows for them (`shadow_slots`); this
        // grows them only after a quality change, never on an edit's first patch.
        if self.native.is_some() {
            self.ensure_native_shadow_capacity(self.shadow_slots())?;
        }
        let mut cascade_patches = Vec::with_capacity(cascade_draw.len());
        for draw in &cascade_draw {
            let (Some(rect), true, Some(cascades)) =
                (draw.rect, self.native.is_some(), &self.look.cascades)
            else {
                cascade_patches.push(None);
                continue;
            };
            let parent = (frame.shadows[draw.cascade], cascades.resolution);
            let patch = crate::look::cascades::Patch::new(rect, parent.1, parent.0);
            let buffer = cascades.frames[draw.cascade];
            let mut patch_frame = frame;
            patch_frame.shadows[draw.cascade] = patch.matrix;
            self.gpu
                .upload_queued(buffer, bytemuck::bytes_of(&patch_frame))?;
            self.reserve_native_shadow_passes(patch_slots)?;
            let pass = self.cull_native_shadow_patch(
                super::native::ShadowSource::Cascade(draw.cascade),
                local_capacity + draw.cascade,
                parent,
                &patch,
            )?;
            cascade_patches.push(Some((pass, patch)));
        }
        // Local-light shadows (look::local): this frame's faces are culled as
        // native shadow passes 7+face and drawn as cascade 4+face, whose matrix
        // the table (rewritten every frame) holds: a patch's cropped one.
        let mut local_draws: Vec<(crate::look::local::LocalDraw, usize)> = Vec::new();
        if self.native.is_some()
            && let Some(mut local) = self.look.local.take()
        {
            let result = (|| -> Result<()> {
                let plan = local.cache.plan(
                    &self.content.lights,
                    camera.projection * camera.view,
                    camera.position,
                    local.size,
                    capture || deterministic,
                );
                let patches = plan.draws.iter().filter(|d| d.patch.is_some()).count();
                self.reserve_native_shadow_passes(if patches > 0 {
                    patch_slots
                } else {
                    local_capacity
                })?;
                let mut slot = local_capacity + 4;
                for &draw in &plan.draws {
                    let resolution = local.settings.resolution;
                    let pass = match &draw.patch {
                        Some(patch) => {
                            slot += 1;
                            self.cull_native_shadow_patch(
                                super::native::ShadowSource::LocalFace(draw.face),
                                slot - 1,
                                (draw.matrix, resolution),
                                patch,
                            )?
                        }
                        None => self.cull_native_shadow_pass_with_resolution(
                            draw.face,
                            draw.matrix,
                            resolution,
                        )?,
                    };
                    local_draws.push((draw, pass));
                }
                self.gpu.upload_queued(local.table, &plan.table)?;
                Ok(())
            })();
            self.look.local = Some(local);
            result?;
        }
        // Heightfield horizon (look::horizon): the capture once per content, culled
        // into the last shadow slot; the sun visibility when the sun moves, the sky
        // visibility once per capture. All of it lands only with a completed frame.
        let mut horizon_capture = None;
        let (mut horizon_sun, mut horizon_sky) = (false, false);
        let held = self.look.horizon.as_ref().map(|h| {
            let sun = crate::look::horizon::sun_key(self.content.sun_direction);
            (
                h.capture,
                h.resolution,
                h.frame,
                h.sun_key != Some(sun),
                !h.sky_done,
            )
        });
        if let (Some((held, resolution, buffer, sun, sky)), true) = (held, self.native.is_some()) {
            (horizon_sun, horizon_sky) = (sun, sky);
            if held.is_none() {
                (horizon_sun, horizon_sky) = (false, false);
                if let Some(bounds) = self.native_content_bounds() {
                    let capture = crate::look::horizon::Capture::new(bounds);
                    let mut capture_frame = frame;
                    capture_frame.shadows[0] = capture.matrix;
                    self.gpu
                        .upload_queued(buffer, bytemuck::bytes_of(&capture_frame))?;
                    let slots = self.shadow_slots();
                    self.reserve_native_shadow_passes(slots)?;
                    let pass = self.cull_native_landforms(
                        slots - 1,
                        capture.matrix,
                        resolution,
                        crate::look::horizon::MIN_RADIUS,
                    )?;
                    horizon_capture = Some((pass, capture));
                    (horizon_sun, horizon_sky) = (true, true);
                }
            }
        }
        // Virtual shadow pages (look::vsm): marks of completed frames become dirty
        // pages; those within the budget whose casters fit draw into the atlas
        // before shading, which falls back to the sun maps until a page lands.
        let (mut vsm_draws, mut vsm_request) = (Vec::new(), None);
        if self.native.is_some()
            && let Some(mut vsm) = self.look.vsm.take()
        {
            let result = (|| -> Result<()> {
                vsm_draws = self.native_shadow_pages(&mut vsm, capture || deterministic)?;
                vsm_request = vsm.arm(&mut self.gpu, camera.position)?;
                Ok(())
            })();
            self.look.vsm = Some(vsm);
            result?;
        }
        self.look.ensure(&mut self.gpu)?;
        let (dof, motion) = self.look.blurs(self.native.is_some());
        let lit = mode.lit();
        // Focus blur needs a perspective lens (an orthographic projection has no w).
        self.look.frame_dof = dof && lit && camera.projection.z_axis.w != 0.;
        self.look.frame_motion = motion && lit;
        // Edges come from the native draws (legacy content has none).
        self.look.frame_wire = mode.wire() && self.native.is_some();
        let (sampled, storage) = self.look.indices();
        // Exposure snaps to its meter on a reset unless a host seeded it, and
        // while new probe GI converges or fades in (no adaptation swing as the
        // scene brightens under it).
        let reset = self.targets.history.is_none() || self.look.settle > 0;
        let seeded = std::mem::take(&mut self.look.seeded);
        let arriving = self.look.gi.as_ref().is_some_and(|gi| gi.arriving());
        let look_frame = LookFrame::new(
            &self.look.effective(),
            &FrameInputs {
                sun_lux: self.content.sun_lux,
                exposure,
                reset,
                snap: (reset && !seeded) || arriving,
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
        if let Some(rows) = self
            .look
            .horizon
            .as_ref()
            .and_then(|h| h.rows(horizon_capture.map(|(_, c)| c), horizon_sun, horizon_sky))
        {
            look_frame.horizon_rect = rows.rect;
            look_frame.horizon_height = rows.height;
            look_frame.horizon_images = rows.images;
            look_frame.horizon_storage = rows.storage;
        }
        // Shading samples the atlas only once it holds a page; native frames with a
        // light space project it into the mask lighting reads.
        let vsm_sampled = self
            .look
            .vsm
            .as_ref()
            .is_some_and(|v| v.written() || !vsm_draws.is_empty());
        let vsm_rows = self
            .look
            .vsm
            .as_ref()
            .filter(|_| self.native.is_some())
            .and_then(|v| v.rows(vsm_request));
        let vsm_project = vsm_rows.is_some();
        if let Some(rows) = vsm_rows {
            look_frame.vsm = rows.vsm;
            if !vsm_sampled {
                look_frame.vsm[1] = 0;
            }
            look_frame.vsm_request = rows.request;
            look_frame.vsm_space = rows.space.to_cols_array_2d();
            look_frame.vsm_depth = rows.depth;
            look_frame.vsm_mask = [
                self.look.sampled[crate::look::gpu::VSM_MASK],
                self.look.storage[crate::look::gpu::VSM_MASK],
                0,
                0,
            ];
        }
        if let Some(local) = &self.look.local
            && self.native.is_some()
        {
            look_frame.local_shadows = [
                local.table_index,
                local.sampled,
                local.size[0],
                local.size[1],
            ];
            if local.settings.in_gi_and_fog {
                look_frame.flags[1] |= crate::look::frame::LOCAL_INDIRECT;
            }
        }
        if let Some(r) = self.look.quality.reflections
            && self.targets.history.is_some()
        {
            look_frame.flags[1] |= crate::look::frame::REFLECTIONS;
            let ssr = self.look.look.ssr;
            look_frame.ssr = [
                r.max_roughness,
                ssr.distance.over(r.distance).max(1.),
                r.steps as f32,
                ssr.intensity,
            ];
            look_frame.ssr_images = self.look.ssr_indices();
        }
        self.rays.write_rows(
            &mut look_frame,
            &self.look.quality,
            camera.projection.y_axis.y,
            self.render,
            self.look.look.ssr.intensity,
        );
        if let Some(ao) = self.look.quality.ambient_occlusion {
            look_frame.flags[1] |= crate::look::frame::AMBIENT_OCCLUSION;
            let tuning = self.look.look.ao;
            look_frame.ao = [
                tuning.radius.over(ao.radius).max(0.05),
                ao.slices as f32,
                ao.steps as f32,
                tuning.intensity,
            ];
            look_frame.ao_images = self.look.ao_indices();
            self.look.ambient_rows(&mut look_frame);
        }
        let contribution = self.look.look.grading.lut_contribution;
        if self.look.lut_size > 0 && contribution > 0. {
            look_frame.lut = [
                self.look.lut_index,
                self.look.lut_size,
                contribution.to_bits(),
                0,
            ];
        }
        let lens = self.look.look.lens;
        if self.look.frame_dof
            && let (Some(quality), Some(focus)) =
                (self.look.quality.depth_of_field, lens.depth_of_field)
        {
            let fov = 2. * (1. / camera.projection.y_axis.y).atan();
            look_frame.dof = focus.constants(fov, self.render[1] as f32, quality);
            look_frame.dof_blur = [focus.far, quality.samples as f32, 0., 0.];
            look_frame.dof_images = self.look.dof_indices();
            look_frame.dof_output = [
                self.look.storage[DOF_COMPOSITE],
                self.look.sampled[DOF_COMPOSITE],
                0,
                0,
            ];
        }
        if self.look.frame_motion
            && let Some(quality) = self.look.quality.motion_blur
        {
            look_frame.motion = [
                lens.motion_blur,
                quality.samples as f32,
                quality.max_length,
                0.,
            ];
            look_frame.motion_images = self.look.motion_indices();
        }
        if let Some(rect) = refract_rect {
            self.look.refraction(&mut self.gpu, &mut look_frame, rect)?;
        }
        // Blended casters' sun shadows (look::translucent) on High/Epic, whose shading
        // and virtual page projection alone compile the lookup.
        let translucent_bounds = self
            .native_translucent_bounds()
            .filter(|_| self.look.quality.high_tier);
        let mut translucent_draw = None;
        if let Some(bounds) = translucent_bounds {
            let (matrix, record) = self.look.translucent_sun(
                &mut self.gpu,
                &mut look_frame,
                std::mem::size_of_val(&frame) as u64,
                bounds,
                self.content.sun_direction,
            )?;
            if let Some(record) = record {
                let mut translucent_frame = frame;
                translucent_frame.shadows[0] = matrix;
                self.gpu
                    .upload_queued(record, bytemuck::bytes_of(&translucent_frame))?;
                self.cull_native_translucent(matrix)?;
                translucent_draw = Some(matrix);
            }
        }
        // Every camera reads the baked reflection captures (look::captures).
        let captures = self.look.quality.reflection_captures.is_some();
        self.views
            .captures
            .write(&mut look_frame, captures, camera.projection * camera.view);
        self.gpu
            .upload_queued(self.look.frame, bytemuck::bytes_of(&look_frame))?;
        let mut graph = Graph::default();
        let mut recipes = Vec::new();
        let ids = self
            .targets
            .images
            .iter()
            .zip(&self.targets.states)
            .map(|(&image, &state)| graph.import(Resource::Image(image), state))
            .collect::<Vec<_>>();
        // GBuffer rewrites these every frame and only this graph reads them: after
        // their last pass, tile-based GPUs neither store nor reload them.
        for target in [ALBEDO, NORMAL, MATERIAL, EMISSIVE] {
            graph.discard(ids[target])?;
        }
        let tile = graph.import(
            Resource::Buffer(self.targets.tiles),
            State {
                access: Access::Storage,
                queue: Queue::Graphics,
            },
        );
        let look = self.look.import(&mut graph);
        let capture_ids = self.views.captures.import(&mut graph, captures);
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
                // The geometry pool stays in Geometry: indexed draws read it as
                // their index buffer as well as through shader storage.
                let read = if access == Access::Geometry {
                    access
                } else {
                    Access::ShaderRead
                };
                graphics.push((id, read));
            });
        }
        let mut frame_input = None;
        for buffer in [self.frame, self.display_frame] {
            let id = graph.import(
                Resource::Buffer(buffer),
                State {
                    access: Access::HostWrite,
                    queue: Queue::Graphics,
                },
            );
            if buffer == self.frame {
                frame_input = Some(id);
            }
            graphics.push((id, Access::ShaderRead));
        }
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
                    .map(|draw| (look.cascades[draw.cascade], Access::Depth)),
            );
            declare(shadow, Recipe::Shadow);
            shadow_passes.push(shadow);
        }
        let mut cascade_reads: Vec<_> = look
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
        // The horizon's capture and visibility maps, before any pass that shades
        // with the sun; they join the sun-map reads once written.
        if let (Some(ids), Some(horizon)) = (look.horizon, &self.look.horizon) {
            if let Some((pass, _)) = horizon_capture {
                let capture = graph.pass(
                    "Horizon.Capture",
                    Queue::Graphics,
                    [(ids[0], Access::Depth)],
                );
                declare(capture, Recipe::HorizonCapture(pass));
                shadow_passes.push(capture);
            }
            for (run, name, pass, image) in [
                (horizon_sun, "Horizon.Sun", LookPass::HorizonSun, ids[1]),
                (horizon_sky, "Horizon.Sky", LookPass::HorizonSky, ids[2]),
            ] {
                if run {
                    let id = graph.pass(
                        name,
                        Queue::Graphics,
                        [
                            (ids[0], Access::ShaderRead),
                            (image, Access::Storage),
                            (look.frame, Access::ShaderRead),
                        ]
                        .into_iter()
                        .chain(graphics.iter().copied()),
                    );
                    declare(id, Recipe::Look(pass));
                }
            }
            for (written, id) in [
                (
                    horizon_capture.is_some() || horizon.capture.is_some(),
                    ids[0],
                ),
                (horizon_sun || horizon.sun_key.is_some(), ids[1]),
                (horizon_sky || horizon.sky_done, ids[2]),
            ] {
                if written {
                    cascade_reads.push((id, Access::ShaderRead));
                }
            }
        }
        // Virtual shadow pages draw with the other sun maps; the atlas joins the
        // sun-map reads once it holds a page.
        if let Some(atlas) = look.vsm.filter(|_| self.native.is_some()) {
            if !vsm_draws.is_empty() {
                let pass = graph.pass("VSM.Pages", Queue::Graphics, [(atlas, Access::Depth)]);
                declare(pass, Recipe::VsmPages);
                shadow_passes.push(pass);
            }
            if vsm_sampled {
                cascade_reads.push((atlas, Access::ShaderRead));
            }
        }
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
        let translucent = translucent_bounds.and_then(|_| self.look.translucent_ids(&mut graph));
        if let (Some([depth, transmittance]), Some(_)) = (translucent, translucent_draw) {
            let pass = graph.pass(
                "TranslucentSun",
                Queue::Graphics,
                [(depth, Access::Depth), (transmittance, Access::Color)],
            );
            declare(pass, Recipe::TranslucentSun);
            shadow_passes.push(pass);
        }
        let translucent_reads: Vec<_> = translucent
            .into_iter()
            .flatten()
            .map(|id| (id, Access::ShaderRead))
            .collect();
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
        // Page visibility for lighting and the pages this frame's pixels sample
        // (look::vsm), read back once the frame completes.
        let mut vsm_mask = None;
        if let (true, Some(vsm), Some(atlas)) = (vsm_project, &self.look.vsm, look.vsm) {
            if let Some(i) = vsm_request {
                let request = graph.import(Resource::Buffer(vsm.requests[i]), host_write);
                let mark = graph.pass(
                    "VSM.Mark",
                    Queue::Graphics,
                    [
                        (ids[DEPTH], Access::ShaderRead),
                        (ids[NORMAL], Access::ShaderRead),
                        (request, Access::Storage),
                        (look.frame, Access::ShaderRead),
                        // Last frame's soft-filter reach (before VSM.Project rewrites it).
                        (look.image(crate::look::gpu::VSM_MASK), Access::ShaderRead),
                    ]
                    .into_iter()
                    .chain(graphics.iter().copied()),
                );
                declare(mark, Recipe::Look(LookPass::VsmMark));
                let readback = graph.pass(
                    "VSM.Requests",
                    Queue::Graphics,
                    [(request, Access::HostRead)],
                );
                declare(readback, Recipe::Empty);
            }
            let mask = look.image(crate::look::gpu::VSM_MASK);
            let project = graph.pass(
                "VSM.Project",
                Queue::Graphics,
                [
                    (ids[DEPTH], Access::ShaderRead),
                    (ids[NORMAL], Access::ShaderRead),
                    (mask, Access::Storage),
                    (look.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(vsm_sampled.then_some((atlas, Access::ShaderRead)))
                .chain(translucent_reads.iter().copied())
                .chain(graphics.iter().copied()),
            );
            declare(project, Recipe::Look(LookPass::VsmProject));
            vsm_mask = Some((mask, Access::ShaderRead));
        }
        let mut ao_reads = graphics.clone();
        ao_reads.extend([
            (ids[DEPTH], Access::ShaderRead),
            (ids[NORMAL], Access::ShaderRead),
        ]);
        let mut ssr_reads = ao_reads.clone();
        ssr_reads.push((ids[MATERIAL], Access::ShaderRead));
        let history = self.targets.history.map(|h| ids[h]);
        let ssr = self.look.ssr_passes(&mut graph, &look, history, &ssr_reads);
        let pyramid = !ssr.is_empty();
        for (id, pass) in ssr {
            declare(id, Recipe::Look(pass));
        }
        // After the reflection pyramid: the High/Epic bitmask bounces last frame's colour.
        // Materials mark cutouts, which occlude nothing.
        for (id, pass) in self.look.ao_passes(&mut graph, &look, &ssr_reads, pyramid) {
            declare(id, Recipe::Look(pass));
        }
        // Fog and GI relight shadow local lights too (look::local).
        let mut sky_shared = graphics.clone();
        sky_shared.extend(cascade_reads.iter().copied());
        sky_shared.extend(far_read);
        sky_shared.extend(local_read);
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
        // Hybrid ray-traced reflections (renderer::rt) read what lighting reads.
        if self.rays.active() {
            let reflected =
                self.look.quality.reflections.is_some() && self.targets.history.is_some();
            let mut ray_reads = gather_reads.clone();
            ray_reads.push((ids[MATERIAL], Access::ShaderRead));
            ray_reads.extend(self.look.lighting_reads(&look, reflected));
            for (id, pass) in self.rays.declare(&mut graph, gbuffer, &ray_reads)? {
                declare(id, Recipe::Rays(pass));
            }
        }
        // This tiny dispatch feeds lighting in the same frame. Keeping it on
        // graphics batches the entire view instead of leaving CPU-recording gaps
        // between graphics/compute submissions and a final ownership handoff.
        let culling = graph.pass("TiledLightCull", Queue::Graphics, [(tile, Access::Storage)]);
        declare(culling, Recipe::Tile);
        let reflected = self.look.quality.reflections.is_some() && self.targets.history.is_some();
        let look_reads = self.look.lighting_reads(&look, reflected);
        // Reflection captures relight against their bakes; then shading reads them.
        let mut capture_reads = Vec::new();
        if let Some(c) = &capture_ids {
            let refit = graph.pass(
                "Captures.Refit",
                Queue::Graphics,
                [(c.records, Access::ShaderRead), (c.frame, Access::Storage)]
                    .into_iter()
                    .chain(look_reads.iter().copied())
                    .chain(cascade_reads.iter().copied())
                    .chain(graphics.iter().copied()),
            );
            let masks = self.views.captures.refit_masks();
            declare(refit, Recipe::Look(LookPass::CaptureRefit(masks)));
            // Each 16x16 tile's captures, from the G-buffer depth.
            let tiles = graph.pass(
                "Captures.Tiles",
                Queue::Graphics,
                [
                    (c.frame, Access::Storage),
                    (ids[DEPTH], Access::ShaderRead),
                    (look.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(graphics.iter().copied()),
            );
            declare(tiles, Recipe::Look(LookPass::CaptureTiles));
            capture_reads = vec![(c.atlas, Access::ShaderRead), (c.frame, Access::ShaderRead)];
        }
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
            .chain(vsm_mask)
            .chain(look_reads.iter().copied())
            .chain(capture_reads.iter().copied())
            .chain(translucent_reads.iter().copied())
            .chain(self.rays.lighting_reads()),
        );
        declare(lighting, Recipe::Lighting);
        let refract_atlas = refract_rect.and_then(|_| {
            let (atlas, passes) = self
                .look
                .refract_passes(&mut graph, look.frame, ids[HDR], ids[DEPTH], &graphics)?;
            for (id, level) in passes {
                declare(id, Recipe::Refract(level));
            }
            Some(atlas)
        });
        // Lit native frames blend colour, ids and motion only, so the G-buffer's
        // last use (and its discard) is lighting; the debug views (and the
        // baseline PSO) also blend into the G-buffer they display.
        let forward_lit = mode.lit() && self.native.is_some();
        let forward_gbuffer = (!forward_lit)
            .then_some([ALBEDO, NORMAL, MATERIAL, EMISSIVE].map(|t| (ids[t], Access::Color)))
            .into_iter()
            .flatten();
        let forward = graph.pass(
            "ForwardTransparency",
            Queue::Graphics,
            [
                (ids[HDR], Access::Color),
                (ids[DEPTH], Access::Depth),
                (ids[ID], Access::Color),
                (tile, Access::ShaderRead),
            ]
            .into_iter()
            .chain(forward_gbuffer)
            .chain(cascade_reads.iter().copied())
            .chain(far_read)
            .chain(local_read)
            .chain(look_reads.iter().copied())
            .chain(self.native.as_ref().map(|_| (ids[MOTION], Access::Color)))
            .chain(capture_reads.iter().copied())
            .chain(refract_atlas.map(|atlas| (atlas, Access::ShaderRead)))
            .chain(translucent_reads.iter().copied())
            // Hybrid frames' TLAS, for forward variants that trace (look-rt.slangh).
            .chain(self.rays.tlas_read()),
        );
        declare(forward, Recipe::Forward(forward_lit));
        let mut shaded = shadow_passes;
        shaded.extend([gbuffer, lighting, forward]);
        if self.look.frame_wire {
            // Depth-tested against the frame's own depth, without writing it.
            let wire = graph.pass(
                "Wire.Draw",
                Queue::Graphics,
                [(ids[WIRE], Access::Color), (ids[DEPTH], Access::Depth)],
            );
            declare(wire, Recipe::Wire);
            shaded.push(wire);
        }
        // Deferred post leaves the lit scene in HDR for external composites (`finish`).
        // Captures always produce their own output, never a deferred frame.
        let deferred = self.look.deferred && mode.lit() && !capture;
        // Ara TSR (crate::tsr) replaces TemporalResolve when selected, for perspective
        // frames of the primary view; a deferred frame plans it here and `finish`
        // declares it. Resolving the primary view without it forgets its history.
        self.tsr.release_unused(&mut self.gpu)?;
        let tsr = self.tsr.upscaler == crate::tsr::Upscaler::Tsr
            && camera.projection.z_axis.w != 0.;
        if tsr {
            self.tsr.prepare(
                &mut self.gpu,
                &self.artifacts,
                &self.shader_metadata.content_key,
                self.render,
                self.size,
            )?;
            self.tsr.begin(&crate::tsr::FrameInputs {
                render: self.render,
                display: self.size,
                upscale: [frame.upscale[0], frame.upscale[1]],
                projection: camera.projection,
                phases: temporal_phases(self.render, self.size),
                reset: self.targets.history.is_none() || deterministic,
                exposure,
                depth: self.targets.sampled[DEPTH],
                motion: self.native.is_some().then(|| self.targets.sampled[MOTION]),
                display_frame: self.display_frame_index,
                exposure_state: self.look.state_index,
                ids: self.targets.sampled[ID],
                // look::refract's copy of the lit opaque scene (level 0) and its rect.
                opaque: refract_atlas
                    .and(refract_rect)
                    .map(|rect| (look_frame.refract_info[0][0], rect)),
                reflections: reflected.then(|| self.look.sampled[crate::look::gpu::SSR_TRACE]),
            })?;
        } else {
            self.tsr.invalidate();
        }
        let mut tsr_ids = None;
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
            // Depth of field composites into the image the resolve reads instead of HDR.
            if self.look.frame_dof {
                for (id, pass) in self
                    .look
                    .dof_passes(&mut graph, &look, ids[HDR], ids[DEPTH], &graphics)
                {
                    declare(id, Recipe::Look(pass));
                }
                taa.push((look.image(DOF_COMPOSITE), Access::ShaderRead));
            }
            if tsr {
                let dof = self.look.frame_dof;
                // TSR's Ara path accumulates into the renderer's history pair.
                let history = self.targets.history.filter(|_| self.tsr.reads_history());
                self.tsr.upload(
                    &mut self.gpu,
                    &crate::tsr::Bindings {
                        colour: if dof {
                            self.look.sampled[DOF_COMPOSITE]
                        } else {
                            self.targets.sampled[HDR]
                        },
                        composited: self.targets.sampled[HDR],
                        output: self.targets.storage[output_history - HISTORY0],
                        history: history.map(|h| self.targets.sampled[h]),
                        reactive: None,
                        opaque: true,
                        reflections: reflected,
                    },
                )?;
                // Prepare inputs reads the engine signals (tsr-signals.slangh).
                let signals = refract_atlas
                    .into_iter()
                    .chain(dof.then_some(ids[HDR]))
                    .chain(reflected.then(|| look.image(crate::look::gpu::SSR_TRACE)))
                    .collect();
                let io = crate::tsr::Io {
                    colour: if dof {
                        look.image(DOF_COMPOSITE)
                    } else {
                        ids[HDR]
                    },
                    depth: ids[DEPTH],
                    motion: self.native.as_ref().map(|_| ids[MOTION]),
                    output: ids[output_history],
                    history: history.map(|h| ids[h]),
                    exposure: look.state,
                    ids: ids[ID],
                    signals,
                };
                let (chain, passes) = self.tsr.declare(&mut graph, &io, &graphics)?;
                for (id, pass) in passes {
                    declare(id, Recipe::Tsr(pass));
                }
                tsr_ids = Some(chain);
            } else {
                let temporal = graph.pass("TemporalResolve", Queue::Graphics, taa);
                declare(temporal, Recipe::Taa);
                shaded.push(temporal);
            }
        }
        if mode.lit() && !deferred {
            for (id, pass) in self.look.post_passes(
                &mut graph,
                &look,
                ids[output_history],
                ids[OUTPUT],
                self.look.frame_motion.then(|| [ids[MOTION], ids[DEPTH]]),
                &graphics,
            ) {
                declare(id, Recipe::Look(pass));
            }
        } else if !mode.lit() {
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
        if self.look.frame_wire && !deferred {
            let composite = graph.pass(
                "Wire.Composite",
                Queue::Graphics,
                [
                    (ids[WIRE], Access::ShaderRead),
                    (ids[ID], Access::ShaderRead),
                    (ids[OUTPUT], Access::Storage),
                ],
            );
            declare(composite, Recipe::WireComposite(mode.lit()));
            shaded.push(composite);
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
            Queue::Graphics,
            [(tile, Access::Storage)],
        );
        declare(ownership, Recipe::Empty);
        for pass in shaded {
            graph.accesses(pass, graphics.iter().copied())?;
        }
        graph.accesses(
            culling,
            [
                (lights, Access::ShaderRead),
                (
                    frame_input.expect("graphics frame imported"),
                    Access::ShaderRead,
                ),
            ],
        )?;
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
        // Native G-buffer and forward fragments read the texture footprint scale
        // from root word 4 (raster.slangh Root).
        let mut footprint = root;
        footprint.words[4] = frame.upscale[2].to_bits();
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
                        for (planned, patch) in cascade_draw.iter().zip(&cascade_patches) {
                            let index = planned.cascade;
                            let cascade = index as u32;
                            // A patch keeps the map and redraws its square: cleared,
                            // then its casters through the cropped matrix.
                            if let (Some((pass, patch)), Some(native)) = (patch, &self.native) {
                                recorder.push(Command::BeginRendering {
                                    colors: Vec::new(),
                                    depth: Some(Attachment {
                                        image: maps.images[index],
                                        clear: None,
                                    }),
                                    size: [maps.resolution; 2],
                                });
                                recorder.push(Command::Viewport {
                                    origin: patch.origin,
                                    size: [patch.size; 2],
                                });
                                recorder.push(Command::ClearDepth {
                                    origin: patch.origin,
                                    size: [patch.size; 2],
                                    depth: 0.,
                                });
                                let mut constants = root;
                                constants.words[0] = maps.frame_indices[index];
                                constants.words[4] = cascade;
                                native.record(&mut recorder, constants, *pass);
                                recorder.push(Command::EndRendering);
                                continue;
                            }
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
                    Recipe::HorizonCapture(pass) => {
                        let horizon = self
                            .look
                            .horizon
                            .as_ref()
                            .expect("a horizon capture is declared only with a horizon");
                        let size = [horizon.resolution; 2];
                        recorder.push(Command::Pipeline(self.pipelines.shadow));
                        recorder.push(Command::BeginRendering {
                            colors: Vec::new(),
                            depth: Some(Attachment {
                                image: horizon.depth,
                                clear: Some([0.; 4]),
                            }),
                            size,
                        });
                        recorder.push(Command::Viewport {
                            origin: [0, 0],
                            size,
                        });
                        let mut constants = root;
                        constants.words[0] = horizon.frame_index;
                        constants.words[4] = 0;
                        if let Some(native) = &self.native {
                            native.record(&mut recorder, constants, pass);
                        }
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::VsmPages => {
                        let (vsm, native) = (
                            self.look
                                .vsm
                                .as_ref()
                                .expect("page draws are declared only with a virtual shadow map"),
                            self.native
                                .as_ref()
                                .expect("page draws are declared only with native content"),
                        );
                        recorder.push(Command::BeginRendering {
                            colors: Vec::new(),
                            depth: Some(Attachment {
                                image: vsm.atlas,
                                clear: None,
                            }),
                            size: vsm.size(),
                        });
                        // Each page clears its square, then draws its casters through
                        // its light view (look::vsm PAGE_DRAWS + i).
                        let page = [ara_vsm::PAGE_SIZE; 2];
                        for (i, draw) in vsm_draws.iter().enumerate() {
                            let origin = vsm.origin(draw.page.slot);
                            recorder.push(Command::Viewport { origin, size: page });
                            recorder.push(Command::ClearDepth {
                                origin,
                                size: page,
                                depth: 0.,
                            });
                            let mut constants = root;
                            constants.words[4] = crate::look::vsm::PAGE_DRAWS + i as u32;
                            native.record_page(&mut recorder, constants, vsm.page_rows_index, i);
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
                        for &(draw, pass) in &local_draws {
                            // A patch clears and redraws its square of the tile.
                            let [x, y] = crate::look::local::origin(draw.face, tile);
                            let (origin, size) = draw.patch.map_or(([x, y], tile), |patch| {
                                ([x + patch.origin[0], y + patch.origin[1]], patch.size)
                            });
                            recorder.push(Command::Viewport {
                                origin,
                                size: [size; 2],
                            });
                            recorder.push(Command::ClearDepth {
                                origin,
                                size: [size; 2],
                                depth: 0.,
                            });
                            let mut constants = root;
                            constants.words[4] = 4 + draw.face as u32;
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
                            size: self.render,
                        });
                        if let Some(native) = &self.native {
                            let detail = self.look.quality.terrain_detail;
                            native.record_gbuffer(&mut recorder, footprint, detail);
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
                        recorder.push(Command::Constants(root));
                        recorder.push(Command::Dispatch([
                            (self.render[0].div_ceil(16) * self.render[1].div_ceil(16))
                                .div_ceil(64),
                            1,
                            1,
                        ]));
                    }
                    Recipe::Lighting => {
                        // Frames that trace bind the variant reading their reflections.
                        let lighting = match self.rays.lighting() {
                            Some(rays) => rays,
                            None if self.look.quality.high_tier => self.pipelines.lighting_high,
                            None => self.pipelines.lighting,
                        };
                        recorder.push(Command::Pipeline(lighting));
                        recorder.push(Command::Constants(root));
                        recorder.push(Command::BeginRendering {
                            colors: vec![Attachment {
                                image: self.targets.images[HDR],
                                clear: Some([0.; 4]),
                            }],
                            depth: None,
                            size: self.render,
                        });
                        recorder.push(Command::Fullscreen);
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::Forward(lit) => {
                        recorder.push(Command::Pipeline(self.pipelines.forward));
                        // Attachment order matches the PSOs' targets: colour, id,
                        // then the G-buffer (unless lit), then motion (native).
                        let gbuffer: &[usize] = if lit {
                            &[]
                        } else {
                            &[ALBEDO, NORMAL, MATERIAL, EMISSIVE]
                        };
                        let native_motion = self.native.as_ref().map(|_| MOTION);
                        recorder.push(Command::BeginRendering {
                            colors: [HDR, ID]
                                .iter()
                                .chain(gbuffer)
                                .copied()
                                .chain(native_motion)
                                .map(|target| Attachment {
                                    image: self.targets.images[target],
                                    clear: None,
                                })
                                .collect(),
                            depth: Some(Attachment {
                                image: self.targets.images[DEPTH],
                                clear: None,
                            }),
                            size: self.render,
                        });
                        if let Some(native) = &self.native {
                            native.record_forward(
                                &mut recorder,
                                footprint,
                                lit,
                                self.look.quality.high_tier,
                                self.rays.refract(),
                            );
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
                    Recipe::Wire => {
                        recorder.push(Command::BeginRendering {
                            colors: vec![Attachment {
                                image: self.targets.images[WIRE],
                                clear: Some([0.; 4]),
                            }],
                            depth: Some(Attachment {
                                image: self.targets.images[DEPTH],
                                clear: None,
                            }),
                            size: self.render,
                        });
                        if let Some(native) = &self.native {
                            native.record_wire(&mut recorder, footprint);
                        }
                        recorder.push(Command::EndRendering);
                    }
                    Recipe::WireComposite(lit) => self.record_wire_composite(&mut recorder, lit),
                    Recipe::Tsr(tsr_pass) => self.tsr.record(&mut recorder, tsr_pass),
                    Recipe::Taa | Recipe::Tonemap => {
                        let taa = matches!(recipes[pass.id.0], Recipe::Taa);
                        recorder.push(Command::Pipeline(if taa {
                            self.pipelines.taa
                        } else {
                            self.pipelines.tonemap
                        }));
                        let mut constants = root;
                        constants.words[0] = self.display_frame_index;
                        constants.words[1] = if taa && self.look.frame_dof {
                            self.look.sampled[DOF_COMPOSITE]
                        } else if taa {
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
                        self.frames(),
                        // The reflection pyramid reads last frame's history; post passes
                        // read this frame's.
                        match look_pass {
                            LookPass::SsrPyramid(_) => {
                                self.targets.history.map_or(0, |h| self.targets.sampled[h])
                            }
                            LookPass::Dof(_) => self.targets.sampled[HDR],
                            _ => self.targets.sampled[output_history],
                        },
                        true,
                    ),
                    Recipe::TranslucentSun => {
                        let map = self
                            .look
                            .translucent
                            .as_ref()
                            .expect("declared only with a translucent sun map");
                        let size = [crate::look::translucent::RESOLUTION; 2];
                        let mut constants = root;
                        constants.words[0] = map.frame_index;
                        constants.words[4] = 0;
                        // Nearest film depth, then every film's opacity over a clear of one.
                        for (transmittance, colors, depth) in [
                            (false, Vec::new(), Some(map.depth)),
                            (true, vec![map.transmittance], None),
                        ] {
                            recorder.push(Command::BeginRendering {
                                colors: colors
                                    .into_iter()
                                    .map(|image| Attachment {
                                        image,
                                        clear: Some([1.; 4]),
                                    })
                                    .collect(),
                                depth: depth.map(|image| Attachment {
                                    image,
                                    clear: Some([0.; 4]),
                                }),
                                size,
                            });
                            recorder.push(Command::Viewport {
                                origin: [0, 0],
                                size,
                            });
                            if let Some(native) = &self.native {
                                native.record_translucent(&mut recorder, constants, transmittance);
                            }
                            recorder.push(Command::EndRendering);
                        }
                    }
                    Recipe::Refract(level) => self.look.record_refract(
                        &mut recorder,
                        level,
                        self.frames(),
                        self.targets.sampled[HDR],
                        refract_rect.unwrap_or_default(),
                    ),
                    Recipe::Rays(ray_pass) => {
                        self.rays
                            .record(&mut recorder, ray_pass, self.frame_index, self.render)
                    }
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
        let graphics_point = points
            .iter()
            .copied()
            .filter(|p| p.queue == Queue::Graphics)
            .max_by_key(|p| p.value);
        for point in points {
            self.last_timings.extend(self.gpu.resolve(point)?);
        }
        for (i, id) in ids.iter().enumerate() {
            self.targets.states[i] = compiled.final_states[id.0];
        }
        self.look.finish(&look, &compiled.final_states);
        self.rays.finish(&compiled.final_states);
        if let Some(chain) = &tsr_ids {
            self.tsr.complete(chain, &compiled.final_states);
        }
        if let Some(ids) = &capture_ids {
            self.views.captures.finish(ids, &compiled.final_states);
        }
        if let Some(atlas) = refract_atlas {
            self.look.refract_finish(atlas, &compiled.final_states);
        }
        if let Some(ids) = translucent {
            self.look.translucent_finish(ids, &compiled.final_states);
        }
        if let Some(matrix) = translucent_draw {
            self.look.translucent_drawn(matrix);
        }
        if let (Some(far), Some((matrix, view))) = (&mut self.look.far, far_draw) {
            far.cache.drawn(matrix, view);
        }
        if let Some(local) = &mut self.look.local {
            for (draw, _) in &local_draws {
                local.cache.drawn(draw);
            }
        }
        if let Some(horizon) = &mut self.look.horizon {
            if let Some((_, capture)) = horizon_capture {
                horizon.capture = Some(capture);
            }
            if horizon_sun {
                horizon.sun_key = Some(crate::look::horizon::sun_key(self.content.sun_direction));
            }
            horizon.sky_done |= horizon_sky;
        }
        if let (Some(vsm), Some(point)) = (&mut self.look.vsm, graphics_point) {
            vsm.submitted(&vsm_draws, vsm_request, point);
        }
        if let Some(cascades) = &mut self.look.cascades {
            for (&draw, patch) in cascade_draw.iter().zip(&cascade_patches) {
                // A patch that could not be culled (legacy content) drew whole.
                let draw = crate::look::cascades::CascadeDraw {
                    rect: patch.and(draw.rect),
                    ..draw
                };
                cascades.drawn(draw, frame.shadows[draw.cascade]);
            }
        }
        if let Some(gi) = &mut self.look.gi {
            gi.advance(self.look.quality.gi_budget);
        }
        self.probe_writes_scattered()?;
        self.look.settle = self.look.settle.saturating_sub(1);
        // A deferred frame keeps the previous history for `finish` to resolve against.
        if deferred {
            self.look.deferred_history = Some(output_history);
        } else {
            self.look.deferred_history = None;
            self.targets.history = Some(output_history);
        }
        self.previous_vp = camera.projection * camera.view;
        self.frame_number = self.frame_number.wrapping_add(1);
        self.gpu.assert_clean()?;
        if let Some(native) = &mut self.native {
            native.presented(&camera, !self.secondary_active);
        }
        if !deferred {
            self.publish_sync_scene()?;
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
    /// it grades `color` directly, which needs a native (not upscaled) frame.
    /// Reuses the frame's constants; returns the SDR output in CopySource.
    pub fn finish(&mut self, color: Image, sampled: u32, state: State) -> Result<Image> {
        self.finish_reactive(color, sampled, state, None)
    }

    /// [`Self::finish`] with the composite's reactive coverage (`reactive`: image,
    /// sampled index and state; render size, R in 0..1, e.g. particle alpha): Ara TSR
    /// trusts the current frame there (FSR's reactive mask) instead of history. TAAU
    /// ignores it. The image is left in Graphics ShaderRead.
    pub fn finish_reactive(
        &mut self,
        color: Image,
        sampled: u32,
        state: State,
        reactive: Option<(Image, u32, State)>,
    ) -> Result<Image> {
        ensure!(
            self.look.deferred_history.is_some() || self.render == self.size,
            "An upscaled frame is graded only through its temporal resolve"
        );
        self.look.ensure(&mut self.gpu)?;
        let mut graph = Graph::default();
        let input = graph.import(Resource::Image(color), state);
        let output = graph.import(
            Resource::Image(self.targets.images[OUTPUT]),
            self.targets.states[OUTPUT],
        );
        let slot = self.look.deferred_history.take();
        // The frame planned Ara TSR (`render_frame`): its chain resolves into the slot.
        let tsr = slot.is_some() && self.tsr.planned();
        let mut imported = Vec::new();
        let mut import = |graph: &mut Graph, target: usize| {
            let id = graph.import(
                Resource::Image(self.targets.images[target]),
                self.targets.states[target],
            );
            imported.push((target, id));
            id
        };
        let depth = import(&mut graph, DEPTH);
        // Motion blur and TSR (native motion) read the motion target.
        let motion = (self.look.frame_motion || (tsr && self.native.is_some()))
            .then(|| import(&mut graph, MOTION));
        let vectors = motion.filter(|_| self.look.frame_motion);
        // The wire composite and TSR read entity IDs.
        let entity_ids = (self.look.frame_wire || tsr).then(|| import(&mut graph, ID));
        let wire_inputs = entity_ids
            .filter(|_| self.look.frame_wire)
            .map(|ids| (import(&mut graph, WIRE), ids));
        let reactive = reactive
            .filter(|_| tsr)
            .map(|(image, index, state)| (graph.import(Resource::Image(image), state), index));
        // Focus blur runs before a resolve; a frame graded directly keeps it sharp.
        let dof = self.look.frame_dof && slot.is_some();
        let resolve = slot.map(|slot| {
            let mut uses = vec![
                (input, Access::ShaderRead),
                (depth, Access::ShaderRead),
                (import(&mut graph, slot), Access::Storage),
            ];
            // TAAU and TSR's Ara path read last frame's history; FSR's keeps its own.
            let reads = !tsr || self.tsr.reads_history();
            let history = self
                .targets
                .history
                .filter(|_| reads)
                .map(|history| (history, import(&mut graph, history)));
            uses.extend(history.map(|(_, id)| (id, Access::ShaderRead)));
            (slot, uses, history)
        });
        let look = self.look.import(&mut graph);
        let frame = graph.import(
            Resource::Buffer(self.display_frame),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        let dof_passes = if dof {
            self.look.dof_passes(
                &mut graph,
                &look,
                input,
                depth,
                &[(frame, Access::ShaderRead)],
            )
        } else {
            Vec::new()
        };
        let (mut tsr_chain, mut tsr_passes) = (None, Vec::new());
        let resolved = match resolve {
            Some((slot, uses, history)) => {
                let slot_id = imported
                    .iter()
                    .find(|(target, _)| *target == slot)
                    .map(|(_, id)| *id)
                    .expect("the resolve imported its history slot");
                // With Ara TSR the resolve is its chain; the last pass (accumulate,
                // or RCAS on FSR's path) writes the slot. Its passes are recorded
                // before the TemporalResolve arm.
                let id = if tsr {
                    // As planned by render_frame; the opaque copy is that graph's.
                    let reflected =
                        self.look.quality.reflections.is_some() && self.targets.history.is_some();
                    self.tsr.upload(
                        &mut self.gpu,
                        &crate::tsr::Bindings {
                            colour: if dof {
                                self.look.sampled[DOF_COMPOSITE]
                            } else {
                                sampled
                            },
                            composited: sampled,
                            output: self.targets.storage[slot - HISTORY0],
                            history: history.map(|(h, _)| self.targets.sampled[h]),
                            reactive: reactive.map(|(_, index)| index),
                            opaque: false,
                            reflections: reflected,
                        },
                    )?;
                    let signals = dof
                        .then_some(input)
                        .into_iter()
                        .chain(reflected.then(|| look.image(crate::look::gpu::SSR_TRACE)))
                        .chain(reactive.map(|(id, _)| id))
                        .collect();
                    let io = crate::tsr::Io {
                        colour: if dof {
                            look.image(DOF_COMPOSITE)
                        } else {
                            input
                        },
                        depth,
                        motion: motion.filter(|_| self.native.is_some()),
                        output: slot_id,
                        history: history.map(|(_, id)| id),
                        exposure: look.state,
                        ids: entity_ids.ok_or_else(|| anyhow::anyhow!("TSR imports entity IDs"))?,
                        signals,
                    };
                    let (chain, passes) =
                        self.tsr
                            .declare(&mut graph, &io, &[(frame, Access::ShaderRead)])?;
                    let last = passes
                        .last()
                        .map(|(id, _)| *id)
                        .ok_or_else(|| anyhow::anyhow!("TSR declares its passes"))?;
                    (tsr_chain, tsr_passes) = (Some(chain), passes);
                    last
                } else {
                    graph.pass(
                        "TemporalResolve",
                        Queue::Graphics,
                        uses.into_iter()
                            .chain([
                                (frame, Access::ShaderRead),
                                (look.state, Access::ShaderRead),
                            ])
                            .chain(dof.then(|| (look.image(DOF_COMPOSITE), Access::ShaderRead))),
                    )
                };
                Some((id, slot, slot_id))
            }
            None => None,
        };
        let (post_input, post_sampled) = match resolved {
            Some((_, slot, slot_id)) => (slot_id, self.targets.sampled[slot]),
            None => (input, sampled),
        };
        let mut passes = self.look.post_passes(
            &mut graph,
            &look,
            post_input,
            output,
            vectors.map(|vectors| [vectors, depth]),
            &[(frame, Access::ShaderRead)],
        );
        passes.extend(dof_passes);
        // Deferred frames are lit: their edges go over the lit image.
        let wire = wire_inputs.map(|(mask, id)| {
            graph.pass(
                "Wire.Composite",
                Queue::Graphics,
                [
                    (mask, Access::ShaderRead),
                    (id, Access::ShaderRead),
                    (output, Access::Storage),
                    (frame, Access::ShaderRead),
                ],
            )
        });
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
                if let Some((_, tsr_pass)) = tsr_passes.iter().find(|(id, _)| *id == pass.id) {
                    self.tsr.record(&mut recorder, *tsr_pass);
                } else if let Some((_, look_pass)) = passes.iter().find(|(id, _)| *id == pass.id) {
                    // Depth of field reads the composited scene; the post chain its resolve.
                    let input = match look_pass {
                        LookPass::Dof(_) => sampled,
                        _ => post_sampled,
                    };
                    self.look.record(
                        &mut recorder,
                        *look_pass,
                        self.frames(),
                        input,
                        resolved.is_some(),
                    );
                } else if wire == Some(pass.id) {
                    self.record_wire_composite(&mut recorder, true);
                } else if resolved.is_some_and(|(id, _, _)| id == pass.id) {
                    let taa_input = if dof {
                        self.look.sampled[DOF_COMPOSITE]
                    } else {
                        sampled
                    };
                    recorder.push(Command::Pipeline(self.pipelines.taa));
                    recorder.push(Command::Constants(RootConstants {
                        words: [self.display_frame_index, taa_input, 0, 0, 0, 0, 0, 0],
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
        if let Some(chain) = &tsr_chain {
            self.tsr.complete(chain, &compiled.final_states);
        }
        self.gpu.assert_clean()?;
        self.publish_sync_scene()?;
        Ok(self.targets.images[OUTPUT])
    }
}

impl Renderer {
    /// Records the wire mask's composite over the display output (`lit`: over the lit
    /// image in one colour; otherwise per-entity colours over the wireframe view).
    fn record_wire_composite(&self, recorder: &mut ara_gpu::vulkan::Recorder, lit: bool) {
        recorder.push(Command::Pipeline(self.pipelines.wire_composite));
        let mut words = [0; 8];
        words[0] = self.display_frame_index;
        words[1] = self.targets.sampled[WIRE];
        words[2] = self.targets.sampled[ID];
        words[5] = u32::from(lit);
        recorder.push(Command::Constants(RootConstants { words }));
        recorder.push(Command::Dispatch([
            self.size[0].div_ceil(8),
            self.size[1].div_ceil(8),
            1,
        ]));
    }

    /// The frame records look passes record with (`look::passes::Frames`).
    fn frames(&self) -> Frames {
        Frames {
            render: (self.frame_index, self.render),
            display: (self.display_frame_index, self.size),
        }
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
