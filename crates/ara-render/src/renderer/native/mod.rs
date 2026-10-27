//! Native raster content: local geometry, compressed residency and draw instances.
use super::*;
use crate::instanced::{GeometryGpu, Scene as NativeScene, TextureStats, Textures, ValidatedScene};
use anyhow::Context;
use ara_gpu::{
    RootConstants,
    commands::{Command, Recorder},
};
use bytemuck::{Pod, Zeroable};
use glam::Vec4;

mod batches;
mod culling;
mod environment;
pub(in crate::renderer) mod pipelines;
mod prepared;
pub use prepared::{NativeContent, NativeSeed};
#[cfg(test)]
mod tests;
mod updates;
mod view;
use batches::Groups;
use culling::{Culling, PageView};
pub(in crate::renderer) use view::NativeView;

/// The cached shadow list a patch redraws part of (`Renderer::cull_native_shadow_patch`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShadowSource {
    Cascade(usize),
    /// A local-light face's reserved slot.
    LocalFace(usize),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRow {
    meta: [u32; 4],
    world: Mat4,
    previous: Mat4,
    normal: [[f32; 4]; 3],
}

pub(super) struct Native {
    pool: GeometryGpu,
    textures: Box<dyn Textures>,
    texture_digest: [u8; 32],
    geometry_revisions: Vec<u64>,
    view_revision: u64,
    materials: Buffer,
    material_index: u32,
    material_data: Vec<crate::instanced::Material>,
    instances: Buffer,
    instance_index: u32,
    visible: Buffer,
    visible_index: u32,
    /// Passes `visible` holds rows for; reservations up to it allocate nothing.
    capacity: usize,
    rows: Vec<InstanceRow>,
    presented: Vec<Mat4>,
    previous_vp: Option<Mat4>,
    bounds: Vec<[Vec3; 2]>,
    groups: Groups,
    culling: Culling,
    changes: updates::Updates,
    pipelines: pipelines::NativePipelines,
    upload: TextureStats,
    material_dirty: bool,
    update_gpu_ms: f64,
}

impl Renderer {
    /// Whether this view draws native local geometry and compressed materials.
    pub fn is_native(&self) -> bool {
        self.native.is_some()
    }
    /// Poll candidate allocations without touching the current generation or
    /// submitting commands. Call at a content boundary, then publish when ready.
    pub fn preallocate_native_textures(&mut self, textures: &mut dyn Textures) -> Result<bool> {
        textures.preallocate(
            &mut self.gpu,
            self.native.as_ref().map(|native| native.textures.as_ref()),
        )
    }
    /// Release native geometry/residency when an explicitly RT-enabled view
    /// switches back to the legacy diagnostic renderer on the same device.
    pub fn clear_native(&mut self) -> Result<()> {
        if let Some(native) = self.native.take() {
            native.destroy(&mut self.gpu)?;
        }
        self.reset_temporal_history();
        Ok(())
    }
    /// Upload native local geometry and compressed texture residency on a raster-only
    /// device. Instances share source allocations; no acceleration structures exist.
    pub fn new_native(
        scene: &NativeScene,
        textures: Box<dyn Textures>,
        size: [u32; 2],
        extensions: &[std::ffi::CString],
        cache: impl Into<PathBuf>,
    ) -> Result<Self> {
        let validated = scene.validated()?;
        let environment = environment::snapshot(scene);
        let mut renderer = Self::new_device(environment, size, extensions, cache.into(), true)?;
        {
            ara_gpu::cpu_scope!("RasterNative.publish");
            renderer.publish_native(validated, textures)?;
        }
        Ok(renderer)
    }

    /// Publish a complete native content generation. Preparation failure keeps the
    /// previous geometry, textures and presentation; moving instances use `update_native`.
    pub fn replace_native_scene(
        &mut self,
        scene: &NativeScene,
        textures: Box<dyn Textures>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.publish");
        let validated = match scene.validated() {
            Ok(validated) => validated,
            Err(error) => {
                textures.destroy(&mut self.gpu)?;
                return Err(error);
            }
        };
        self.publish_native(validated, textures)
    }

    fn publish_native(
        &mut self,
        validated: ValidatedScene<'_>,
        textures: Box<dyn Textures>,
    ) -> Result<()> {
        self.publish_native_pool(validated, textures, None, self.shadow_slots())
    }

    fn publish_native_pool(
        &mut self,
        validated: ValidatedScene<'_>,
        textures: Box<dyn Textures>,
        pool: Option<GeometryGpu>,
        shadow_slots: usize,
    ) -> Result<()> {
        let scene = validated.scene();
        let pipelines = match self.native_pipeline_handles() {
            Ok(pipelines) => pipelines,
            Err(error) => {
                if let Some(pool) = pool {
                    pool.destroy(&mut self.gpu)?;
                }
                textures.destroy(&mut self.gpu)?;
                return Err(error);
            }
        };
        let mut next = Native::new(
            &mut self.gpu,
            validated,
            textures,
            pipelines,
            self.native.as_ref(),
            pool,
            shadow_slots,
        )?;
        // The first native generation replaces bootstrap/legacy geometry. HDRI
        // changes rebuild the actual environment and its IBL, rather than merely
        // changing the capture metadata. Ordinary motion never enters this path.
        let environment = environment::snapshot(scene);
        let content = if !self.content.meshes.is_empty()
            || super::hash::static_hash(&environment) != self.content.static_hash
        {
            let content = match upload::upload_content(
                &mut self.gpu,
                environment,
                &self.artifacts,
                &self.shader_metadata,
                self.compute_frame,
                Some(self.content.ibl_pipelines),
            ) {
                Ok(content) => content,
                Err(error) => {
                    next.destroy(&mut self.gpu)?;
                    return Err(error);
                }
            };
            Some(content)
        } else {
            None
        };
        let mut hash = sha2::Sha256::new();
        use sha2::Digest;
        hash.update(
            content
                .as_ref()
                .map_or(self.content.static_hash.as_str(), |content| {
                    content.static_hash.as_str()
                })
                .as_bytes(),
        );
        for geometry in &scene.geometry {
            hash.update(geometry.key);
        }
        let base_hash = format!("{:x}", hash.finalize());
        let names = scene
            .instances
            .iter()
            .map(|i| (i.entity, i.name.to_owned()))
            .collect();
        if let Some(previous) = &mut self.native
            && let Err(error) = next.textures.publish_reuse(previous.textures.as_mut())
        {
            next.destroy(&mut self.gpu)?;
            if let Some(content) = content {
                self.retire_content(content)?;
            }
            return Err(error);
        }
        let old = self.native.replace(next);
        self.entity_epoch = self.entity_epoch.wrapping_add(1);
        let previous_content = content.map(|content| std::mem::replace(&mut self.content, content));
        self.content.base_hash = base_hash;
        self.content.entity_names = names;
        self.content.revision += 1;
        self.targets.history = None;
        if let Some(previous) = previous_content {
            self.retire_content(previous)?;
        }
        if let Some(old) = old {
            old.destroy(&mut self.gpu)?;
        }
        Ok(())
    }

    /// Update current transforms and changed skin poses without replacing shared
    /// geometry or textures. Structural edits require `replace_native_scene`.
    pub fn update_native(&mut self, scene: &NativeScene) -> Result<()> {
        self.update_native_inner(scene, None)
    }

    /// Apply publication-local dirty rows. The producer must report every changed
    /// instance, material and skin; use `update_native` for a snapshot without a
    /// change list. Structural/layout edits still require scene replacement.
    pub fn update_native_changed(
        &mut self,
        scene: &NativeScene,
        changes: &crate::instanced::Changes,
    ) -> Result<()> {
        self.update_native_inner(scene, Some(changes))
    }

    fn update_native_inner(
        &mut self,
        scene: &NativeScene,
        changes: Option<&crate::instanced::Changes>,
    ) -> Result<()> {
        ensure!(
            scene.lights.len() <= upload::MAX_LIGHTS
                && bytemuck::cast_slice::<_, f32>(&scene.lights)
                    .iter()
                    .all(|v| v.is_finite()),
            "Invalid native light list"
        );
        ensure!(
            scene.sun_direction.is_finite()
                && scene.sun_direction.length_squared() > 0.001
                && scene.sun_lux.is_finite(),
            "Invalid directional light"
        );
        let native = self
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Native raster content required"))?;
        native.update(&mut self.gpu, scene, changes)?;
        if bytemuck::cast_slice::<_, u8>(&self.content.lights)
            != bytemuck::cast_slice::<_, u8>(&scene.lights)
        {
            let empty = [crate::scene::Light::zeroed()];
            let lights = if scene.lights.is_empty() {
                &empty
            } else {
                &scene.lights[..]
            };
            for buffer in [self.content.light_buffer, self.content.compute_lights] {
                self.gpu
                    .upload_queued(buffer, bytemuck::cast_slice(lights))?;
            }
            self.content.lights.clear();
            self.content.lights.extend_from_slice(&scene.lights);
        }
        if self.content.sun_direction != scene.sun_direction
            || self.content.sun_lux != scene.sun_lux
        {
            self.set_sun(scene.sun_direction, scene.sun_lux)?;
        }
        self.content.revision += 1;
        Ok(())
    }

    /// Dirty caster bounds from the most recently prepared native frame. New
    /// content invalidates all shadows; a settled idle frame reports no bounds.
    pub fn native_caster_changes(&self) -> Option<crate::instanced::CasterChanges<'_>> {
        self.native
            .as_ref()
            .map(|native| crate::instanced::CasterChanges {
                all: native.changes.caster_all,
                bounds: &native.changes.caster_bounds,
            })
    }

    /// CPU traversal and upload counters for the last prepared native frame.
    pub fn native_frame_work(&self) -> Option<crate::instanced::FrameWork> {
        self.native.as_ref().map(|native| native.changes.work)
    }

    /// Prepare opaque (including alpha-masked) casters against an arbitrary light
    /// matrix, using its shadow texel density. Call after native preparation
    /// and before declaring the shadow pass resources; record it as native pass 6.
    pub fn cull_native_shadow(&mut self, matrix: Mat4) -> Result<()> {
        let shadows = self.look.quality.shadows;
        let resolution = shadows.far.map_or(shadows.resolution, |far| far.resolution);
        self.cull_native_shadow_with_resolution(matrix, resolution)
    }

    /// Cull far casters with the actual square shadow viewport resolution.
    /// Bound radii below two texels (or the preset minimum) are omitted.
    pub fn cull_native_shadow_with_resolution(
        &mut self,
        matrix: Mat4,
        resolution: u32,
    ) -> Result<()> {
        ensure!(
            matrix.is_finite() && resolution > 0,
            "Invalid native shadow view"
        );
        let view = culling::View::shadow(matrix, resolution, self.look.quality.min_pixels);
        let native = self
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Native raster content required"))?;
        native.culling.opaque(
            &mut self.gpu,
            native.visible,
            &native.groups,
            &native.bounds,
            6,
            view,
            &mut native.changes.work,
        )
    }

    /// Shadow slots this quality culls into: its local faces, then one patch per
    /// cascade, `look::local::PATCHES` face patches and the horizon capture (the
    /// last; renderer/passes.rs).
    pub(crate) fn shadow_slots(&self) -> usize {
        Self::shadow_slots_for_quality(&self.look.quality)
    }

    pub(crate) fn shadow_slots_for_quality(quality: &crate::look::Quality) -> usize {
        quality
            .local_shadows
            .map_or(0, crate::look::local::capacity)
            + 4
            + crate::look::local::PATCHES
            + 1
    }

    /// World bounds (min, max) of every native instance; `None` without native
    /// content.
    pub(crate) fn native_content_bounds(&self) -> Option<[Vec3; 2]> {
        let native = self.native.as_ref()?;
        native
            .bounds
            .iter()
            .copied()
            .reduce(|[lo, hi], [a, b]| [lo.min(a), hi.max(b)])
    }

    /// Cull the landforms of a heightfield capture (look::horizon) into reserved
    /// slot `slot`: opaque casters with bound radii of at least `min_radius` metres
    /// at the view's texel density, with the LODs a shadow view of `resolution`
    /// texels picks.
    pub(crate) fn cull_native_landforms(
        &mut self,
        slot: usize,
        matrix: Mat4,
        resolution: u32,
        min_radius: f32,
    ) -> Result<usize> {
        ensure!(
            matrix.is_finite() && resolution > 0 && min_radius.is_finite(),
            "Invalid native landform view"
        );
        let texel = 2. / (matrix.row(0).truncate().length() * resolution as f32);
        let view = culling::View::shadow(matrix, resolution, min_radius / texel);
        self.cull_native_slot(slot, view)
    }

    /// Size this camera's culling rows for `count` shadow slots without reserving
    /// them, so later reservations up to it (shadow patches after an edit)
    /// allocate nothing. Call before graph resource declarations; growth is
    /// transactional and O(1) once sized.
    pub(crate) fn ensure_native_shadow_capacity(&mut self, count: usize) -> Result<()> {
        let native = self
            .native
            .as_mut()
            .context("Native raster content required")?;
        native.ensure_shadow_capacity(&mut self.gpu, count)
    }

    /// Reserve independently cached local-light shadow lists. Call before graph
    /// resource declarations; growth preserves the camera/cascade/far lists and
    /// is transactional on GPU allocation/upload failure. No storage is allocated
    /// for local shadows until requested (or sized by `ensure_native_shadow_capacity`),
    /// and repeated reservations are O(1).
    pub fn reserve_native_shadow_passes(&mut self, count: usize) -> Result<()> {
        let native = self
            .native
            .as_mut()
            .context("Native raster content required")?;
        native.reserve_shadow_passes(&mut self.gpu, count)
    }

    /// Cull opaque/masked casters for one reserved local-light face using the
    /// local-light texel budget. Use `cull_native_shadow_pass_with_resolution` when
    /// the local-light tile has a different resolution. Call after native
    /// preparation and before graph resource declarations.
    /// The renderer records the returned pass with its main Frame, cascade word
    /// 4+slot and the look's local-matrix table. External passes may instead bind
    /// their own Frame with this matrix in shadows[0] and cascade zero. Lists
    /// remain independent of each other and of far-shadow pass 6.
    /// Arbitrary perspective and orthographic matrices test all six clip planes.
    /// Unculled slots keep their lists without traversal or visible-buffer writes.
    pub fn cull_native_shadow_pass(&mut self, slot: usize, matrix: Mat4) -> Result<usize> {
        let resolution = self
            .look
            .quality
            .local_shadows
            .map_or(self.look.quality.shadows.resolution, |local| {
                local.resolution
            });
        self.cull_native_shadow_pass_with_resolution(slot, matrix, resolution)
    }

    /// Cull a reserved local face with its actual square tile resolution. Matrix,
    /// resolution and preset changes invalidate only the affected cached list.
    pub fn cull_native_shadow_pass_with_resolution(
        &mut self,
        slot: usize,
        matrix: Mat4,
        resolution: u32,
    ) -> Result<usize> {
        ensure!(
            matrix.is_finite() && resolution > 0,
            "Invalid native shadow view"
        );
        let view = culling::View::shadow(matrix, resolution, self.look.quality.min_pixels);
        self.cull_native_slot(slot, view)
    }

    fn cull_native_slot(&mut self, slot: usize, view: culling::View) -> Result<usize> {
        let native = self
            .native
            .as_mut()
            .context("Native raster content required")?;
        let pass = culling::BASE_PASSES
            .checked_add(slot)
            .context("Native shadow slot overflow")?;
        ensure!(
            pass < native.culling.draws.len(),
            "Native shadow slot was not reserved"
        );
        native.culling.opaque(
            &mut self.gpu,
            native.visible,
            &native.groups,
            &native.bounds,
            pass,
            view,
            &mut native.changes.work,
        )?;
        Ok(pass)
    }

    /// Cull a shadow patch (look::cascades, look::local) into reserved slot `slot`:
    /// the instances `source`'s cached list (drawn with `parent`, `resolution`
    /// texels a side) holds under `patch`, plus those moved since, tested against
    /// the patch's cropped matrix at its own size. Call after native preparation
    /// and before graph resource declarations; record the returned pass.
    pub(crate) fn cull_native_shadow_patch(
        &mut self,
        source: ShadowSource,
        slot: usize,
        (parent, resolution): (Mat4, u32),
        patch: &crate::look::cascades::Patch,
    ) -> Result<usize> {
        let view = culling::View::shadow(patch.matrix, patch.size, self.look.quality.min_pixels);
        let native = self
            .native
            .as_mut()
            .context("Native raster content required")?;
        let source = match source {
            ShadowSource::Cascade(cascade) => cascade,
            ShadowSource::LocalFace(face) => culling::BASE_PASSES + face,
        };
        let pass = culling::BASE_PASSES
            .checked_add(slot)
            .context("Native shadow slot overflow")?;
        ensure!(
            pass < native.culling.draws.len() && source < native.culling.draws.len(),
            "Native shadow patch slot was not reserved"
        );
        native.culling.patch(
            &mut self.gpu,
            native.visible,
            &native.groups,
            &native.bounds,
            (source, parent, patch.clip(resolution)),
            pass,
            view,
            &mut native.changes.work,
        )?;
        Ok(pass)
    }

    /// This frame's virtual shadow pages (look::vsm): plans `vsm` against the native
    /// casters' changes, culls each page's opaque casters (through a grid in the
    /// sun's light space) into the map's caster rows until they are full, and
    /// returns the pages that fit (the rest wait); `Native::record_page` draws them.
    /// `settle` (deterministic frames) waits for outstanding page requests.
    pub(crate) fn native_shadow_pages(
        &mut self,
        vsm: &mut crate::look::vsm::VirtualShadowMap,
        settle: bool,
    ) -> Result<Vec<crate::look::vsm::PageDraw>> {
        let Some(native) = self.native.as_mut() else {
            return Ok(Vec::new());
        };
        let Some(content) = native
            .bounds
            .iter()
            .copied()
            .reduce(|[lo, hi], [a, b]| [lo.min(a), hi.max(b)])
        else {
            return Ok(Vec::new());
        };
        let changes = (native.changes.caster_all, &native.changes.caster_bounds[..]);
        let mut draws = vsm.plan(
            &mut self.gpu,
            self.content.sun_direction,
            content,
            changes,
            settle,
        )?;
        let rotation = vsm
            .space
            .context("a planned virtual shadow map has a light space")?
            .rotation;
        let views: Vec<_> = draws
            .iter()
            .map(|draw| {
                let (lo, hi) = draw.rect();
                PageView {
                    matrix: draw.matrix,
                    lo,
                    hi,
                }
            })
            .collect();
        let fit = native.culling.cull_pages(
            &mut self.gpu,
            &native.groups,
            &native.bounds,
            rotation,
            &views,
            (vsm.page_rows, vsm.page_rows_capacity),
            &mut native.changes.work,
        )?;
        let rest = draws.split_off(fit);
        vsm.drawn(&mut self.gpu, &draws, &rest)?;
        Ok(draws)
    }

    /// Compressed texture upload costs of the last completed native frame.
    pub fn native_texture_stats(&self) -> Option<TextureStats> {
        self.native.as_ref().map(|native| native.upload)
    }

    /// Promote compressed native mip tails at a capture/control boundary without
    /// drawing or allocating acceleration structures.
    pub fn refine_native_textures(&mut self) -> Result<Option<TextureStats>> {
        let Some(native) = &mut self.native else {
            return Ok(None);
        };
        native.upload = native.textures.refine(&mut self.gpu)?;
        Ok(Some(native.upload))
    }

    /// Shared geometry payload and optional-AS counts for the native generation.
    pub fn native_geometry_stats(&self) -> Option<crate::instanced::GeometryUpload> {
        self.native.as_ref().map(|native| native.pool.stats())
    }
    /// GPU skin/pose staging from the last native update; rigid motion uses coherent
    /// instance storage and performs no acceleration-structure work.
    pub fn native_update_gpu_ms(&self) -> Option<f64> {
        self.native.as_ref().map(|native| native.update_gpu_ms)
    }
}

impl Native {
    fn reserve_shadow_passes(&mut self, gpu: &mut GpuDevice, count: usize) -> Result<()> {
        let passes = culling::BASE_PASSES
            .checked_add(count)
            .context("Native shadow count overflow")?;
        if passes <= self.culling.draws.len() {
            return Ok(());
        }
        self.ensure_shadow_capacity(gpu, count)?;
        self.culling.reserve(&self.groups, passes);
        Ok(())
    }

    /// Grows `visible` to hold rows for `count` shadow slots (the reserved lists'
    /// rows carry over), leaving the reserved lists as they are.
    fn ensure_shadow_capacity(&mut self, gpu: &mut GpuDevice, count: usize) -> Result<()> {
        let passes = culling::BASE_PASSES
            .checked_add(count)
            .context("Native shadow count overflow")?;
        if passes <= self.capacity {
            return Ok(());
        }
        ara_gpu::cpu_scope!("RasterNative.reserve_shadow_passes");
        let rows = self
            .rows
            .len()
            .checked_mul(passes)
            .context("Native shadow row count overflow")?;
        ensure!(
            rows <= u32::MAX as usize,
            "Native shadow row index exceeds u32"
        );
        let bytes = rows
            .checked_mul(8)
            .context("Native shadow buffer size overflow")?
            .max(16);
        let buffer = gpu.create_buffer(bytes as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let staged = (|| {
            if !self.culling.visible.is_empty() {
                gpu.upload_range(buffer, 0, bytemuck::cast_slice(&self.culling.visible))?;
            }
            Ok::<_, anyhow::Error>(gpu.storage_buffer(buffer)?.index)
        })();
        let index = match staged {
            Ok(index) => index,
            Err(error) => {
                gpu.retire(Resource::Buffer(buffer))?;
                return Err(error);
            }
        };
        let previous = std::mem::replace(&mut self.visible, buffer);
        self.visible_index = index;
        self.capacity = passes;
        gpu.retire(Resource::Buffer(previous))?;
        Ok(())
    }
    /// A content generation whose culling rows hold `shadow_slots` shadow slots
    /// (`Renderer::shadow_slots`) or, when larger, the previous generation's, so
    /// its frames never grow them (a growth retires a buffer mid-session).
    fn new(
        gpu: &mut GpuDevice,
        validated: ValidatedScene<'_>,
        mut textures: Box<dyn Textures>,
        pipelines: pipelines::NativePipelines,
        previous: Option<&Self>,
        mut provided_pool: Option<GeometryGpu>,
        shadow_slots: usize,
    ) -> Result<Self> {
        ara_gpu::cpu_scope!("RasterNative.create_content");
        let scene = validated.scene();
        let texture_digest = textures.content_digest();
        // Local-light face slots belong to the view's quality configuration,
        // which is not re-applied when only a content generation changes.
        let passes = previous.map_or(culling::BASE_PASSES, |old| old.culling.draws.len());
        let capacity = (culling::BASE_PASSES + shadow_slots)
            .max(passes)
            .max(previous.map_or(0, |old| old.capacity));
        let visible_bytes = scene
            .instances
            .len()
            .checked_mul(capacity)
            .filter(|rows| *rows <= u32::MAX as usize)
            .and_then(|rows| rows.checked_mul(8));
        let Some(visible_bytes) = visible_bytes else {
            if let Some(pool) = provided_pool {
                pool.destroy(gpu)?;
            }
            textures.destroy(gpu)?;
            anyhow::bail!("Native shadow visibility capacity overflow");
        };
        let visible_bytes = visible_bytes.max(16);
        let upload = {
            ara_gpu::cpu_scope!("RasterNative.publish_textures");
            let initialized = match previous {
                Some(previous) => textures.initialize_reusing(gpu, previous.textures.as_ref()),
                None => textures.initialize(gpu),
            };
            match initialized {
                Ok(upload) => upload,
                Err(error) => {
                    if let Some(pool) = provided_pool {
                        pool.destroy(gpu)?;
                    }
                    textures.destroy(gpu)?;
                    return Err(error);
                }
            }
        };
        if textures.binding().map_or(0, |b| b.textures) != scene.textures {
            if let Some(pool) = provided_pool {
                pool.destroy(gpu)?;
            }
            textures.destroy(gpu)?;
            anyhow::bail!("Native texture-table count mismatch");
        }
        let pool = {
            ara_gpu::cpu_scope!("RasterNative.publish_geometry");
            let geometry = match provided_pool.take() {
                Some(pool) => Ok(pool),
                None => {
                    GeometryGpu::new_validated(gpu, validated, false, previous.map(|v| &v.pool))
                }
            };
            match geometry {
                Ok(pool) => pool,
                Err(error) => {
                    textures.destroy(gpu)?;
                    return Err(error);
                }
            }
        };
        let mut owned = Vec::new();
        let prepared = (|| {
            let materials = host_buffer(
                gpu,
                (scene.materials.len() * std::mem::size_of::<crate::instanced::Material>()).max(16),
                &mut owned,
            )?;
            gpu.upload(materials, bytemuck::cast_slice(&scene.materials))?;
            let instances = host_buffer(
                gpu,
                128 + scene.instances.len() * std::mem::size_of::<InstanceRow>(),
                &mut owned,
            )?;
            let visible = host_buffer(gpu, visible_bytes, &mut owned)?;
            Ok((
                materials,
                gpu.storage_buffer(materials)?.index,
                instances,
                gpu.storage_buffer(instances)?.index,
                visible,
                gpu.storage_buffer(visible)?.index,
            ))
        })();
        let (materials, material_index, instances, instance_index, visible, visible_index) =
            match prepared {
                Ok(result) => result,
                Err(error) => {
                    pool.destroy(gpu)?;
                    textures.destroy(gpu)?;
                    for resource in owned {
                        gpu.retire(resource)?;
                    }
                    gpu.collect()?;
                    return Err(error);
                }
            };
        let rows = scene.instances.iter().map(|i| row(i, i.world)).collect();
        let groups = Groups::new(scene, &pool)?;
        let mut culling = Culling::new(&groups, scene);
        culling.reserve(&groups, passes);
        let result = Self {
            material_index,
            instance_index,
            visible_index,
            groups,
            pool,
            textures,
            texture_digest,
            geometry_revisions: scene
                .geometry
                .iter()
                .map(|geometry| geometry.revision)
                .collect(),
            view_revision: 0,
            materials,
            instances,
            visible,
            capacity,
            rows,
            material_data: scene.materials.to_vec(),
            presented: scene.instances.iter().map(|i| i.world).collect(),
            previous_vp: None,
            bounds: scene
                .instances
                .iter()
                .map(|i| i.bounds(&scene.geometry[i.geometry]))
                .collect(),
            culling,
            changes: updates::Updates::new(scene.instances.len()),
            pipelines,
            upload,
            material_dirty: true,
            update_gpu_ms: 0.,
        };
        Ok(result)
    }

    pub(super) fn resources(&self, visit: &mut dyn FnMut(Resource, Access)) {
        for (resource, access) in self.pool.resources() {
            visit(resource, access);
        }
        self.textures.visit_resources(visit);
        visit(
            Resource::Buffer(self.materials),
            if self.material_dirty {
                Access::HostWrite
            } else {
                Access::ShaderRead
            },
        );
        visit(Resource::Buffer(self.instances), Access::HostWrite);
        visit(Resource::Buffer(self.visible), Access::HostWrite);
    }

    pub(super) fn record(&self, recorder: &mut Recorder, root: RootConstants, pass: usize) {
        let pipeline = match pass {
            4 => self.pipelines.gbuffer,
            5 => self.pipelines.forward,
            _ => self.pipelines.shadow,
        };
        self.record_with(recorder, root, pass, pipeline);
    }

    /// Records the camera's opaque and forward draws (passes 4 and 5) again through
    /// the wire PSO: the wireframe view modes' edge mask.
    pub(super) fn record_wire(&self, recorder: &mut Recorder, root: RootConstants) {
        for pass in [4, 5] {
            self.record_with(recorder, root, pass, self.pipelines.wire);
        }
    }

    /// Records the camera's forward draws (pass 5) through the High/Epic tier's PSO
    /// when `high` (`Quality::high_tier`).
    pub(super) fn record_forward(&self, recorder: &mut Recorder, root: RootConstants, high: bool) {
        let pipeline = if high {
            self.pipelines.forward_high
        } else {
            self.pipelines.forward
        };
        self.record_with(recorder, root, 5, pipeline);
    }

    /// Records the G-buffer draws (pass 4) through the PSO for the preset's
    /// `terrain_detail` (`NativePipelines::gbuffer_for`).
    pub(super) fn record_gbuffer(
        &self,
        recorder: &mut Recorder,
        root: RootConstants,
        terrain_detail: f32,
    ) {
        let pipeline = self.pipelines.gbuffer_for(terrain_detail);
        self.record_with(recorder, root, 4, pipeline);
    }

    fn record_with(
        &self,
        recorder: &mut Recorder,
        root: RootConstants,
        pass: usize,
        pipeline: Pipeline,
    ) {
        ara_gpu::cpu_scope!("RasterNative.record");
        recorder.push(Command::Pipeline(pipeline));
        for batch in &self.culling.draws[pass] {
            if batch.count == 0 {
                continue;
            }
            let mut constants = root;
            constants.words[1] = self.material_index;
            constants.words[2] = self.instance_index;
            constants.words[3] = batch.first;
            constants.words[5] = self.pool.table_index().unwrap_or(0);
            constants.words[6] = self.textures.binding().map_or(0, |b| b.buffer_index);
            constants.words[7] = self.visible_index;
            recorder.push(Command::Constants(constants));
            recorder.push(Command::DrawProcedural {
                vertices: batch.vertices,
                instances: batch.count,
            });
        }
    }

    /// Records page `page` of the last `cull_native_shadow_pages` with the shadow
    /// pipeline, its rows read from storage buffer `rows` (bindless index).
    pub(super) fn record_page(
        &self,
        recorder: &mut Recorder,
        root: RootConstants,
        rows: u32,
        page: usize,
    ) {
        recorder.push(Command::Pipeline(self.pipelines.shadow));
        for batch in self.culling.page_batches.get(page).into_iter().flatten() {
            let mut constants = root;
            constants.words[1] = self.material_index;
            constants.words[2] = self.instance_index;
            constants.words[3] = batch.first;
            constants.words[5] = self.pool.table_index().unwrap_or(0);
            constants.words[6] = self.textures.binding().map_or(0, |b| b.buffer_index);
            constants.words[7] = rows;
            recorder.push(Command::Constants(constants));
            recorder.push(Command::DrawProcedural {
                vertices: batch.vertices,
                instances: batch.count,
            });
        }
    }

    pub(super) fn hash(&self, hash: &mut sha2::Sha256) {
        use sha2::Digest;
        // Previous presentation poses and skin-history flags describe temporal
        // transport, not content; a settled frame keeps the same scene identity.
        for row in &self.rows {
            hash.update(bytemuck::bytes_of(&row.world));
            hash.update(row.meta[0].to_le_bytes());
            hash.update(bytemuck::cast_slice(&row.meta[2..]));
        }
        hash.update(bytemuck::cast_slice(&self.geometry_revisions));
        hash.update(bytemuck::cast_slice(&self.material_data));
        hash.update(self.texture_digest);
    }

    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.retire_content");
        self.pool.destroy(gpu)?;
        self.textures.destroy(gpu)?;
        for buffer in [self.materials, self.instances, self.visible] {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        gpu.collect()?;
        Ok(())
    }
}

fn host_buffer(gpu: &mut GpuDevice, size: usize, owned: &mut Vec<Resource>) -> Result<Buffer> {
    let buffer = gpu.create_buffer(size as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    owned.push(Resource::Buffer(buffer));
    Ok(buffer)
}

fn row(instance: &crate::instanced::Instance, previous: Mat4) -> InstanceRow {
    let normal = instance.world.inverse().transpose();
    InstanceRow {
        meta: [
            instance.material as u32,
            0,
            0,
            instance.world.determinant().signum().to_bits(),
        ],
        world: instance.world,
        previous,
        normal: [
            normal.row(0).to_array(),
            normal.row(1).to_array(),
            normal.row(2).to_array(),
        ],
    }
}
