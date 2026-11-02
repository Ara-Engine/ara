//! Dirty GPU rows and motion history, independent of static scene size.
use super::*;
use crate::instanced::{Changes, FrameWork};
use anyhow::Context;

pub(super) struct Indices {
    marked: Vec<bool>,
    pub list: Vec<usize>,
}
impl Indices {
    pub fn new(count: usize) -> Self {
        Self {
            marked: vec![false; count],
            list: Vec::new(),
        }
    }
    pub fn insert(&mut self, index: usize) -> bool {
        if self.marked[index] {
            return false;
        }
        self.marked[index] = true;
        self.list.push(index);
        true
    }
    pub fn clear(&mut self) {
        for index in self.list.drain(..) {
            self.marked[index] = false;
        }
    }
}

pub(super) struct Updates {
    rows: Indices,
    settle: Vec<usize>,
    skins: Indices,
    casters: Indices,
    unions: Vec<[Vec3; 2]>,
    /// Per row: the rigid displacement bound (m) of this frame's transform changes;
    /// `None` once a skin pose, material or paint change touched the caster.
    displacement: Vec<Option<f32>>,
    header: Option<[Mat4; 2]>,
    all: bool,
    materials: usize,
    pub caster_all: bool,
    pub caster_bounds: Vec<[Vec3; 2]>,
    /// `caster_bounds`' rows and displacement bounds, in the same order (the sun page
    /// service's dynamic layer, look::vsm::sun; `None`: no bound).
    pub caster_rows: Vec<usize>,
    pub caster_displacement: Vec<Option<f32>>,
    pub work: FrameWork,
}
impl Updates {
    /// A secondary camera adopts the current instance rows at a content boundary.
    pub(super) fn invalidate(&mut self, count: usize) {
        self.rows.clear();
        for index in 0..count {
            self.rows.insert(index);
        }
        self.settle.clear();
        self.skins.clear();
        self.casters.clear();
        self.header = None;
        self.all = true;
    }
    pub fn new(count: usize) -> Self {
        let mut rows = Indices::new(count);
        for i in 0..count {
            rows.insert(i);
        }
        Self {
            rows,
            settle: Vec::new(),
            skins: Indices::new(count),
            casters: Indices::new(count),
            unions: vec![[Vec3::ZERO; 2]; count],
            displacement: vec![None; count],
            header: None,
            all: true,
            materials: 0,
            caster_all: true,
            caster_bounds: Vec::new(),
            caster_rows: Vec::new(),
            caster_displacement: Vec::new(),
            work: FrameWork::default(),
        }
    }
    /// A caster changed in a way no displacement bounds (a pose, material or paint).
    fn caster(&mut self, index: usize, old: [Vec3; 2], new: [Vec3; 2]) {
        self.union(index, old, new);
        self.displacement[index] = None;
    }
    /// A caster moved rigidly, no point by more than `displacement` metres.
    fn caster_moved(&mut self, index: usize, old: [Vec3; 2], new: [Vec3; 2], displacement: f32) {
        let first = self.union(index, old, new);
        self.displacement[index] = if first {
            Some(displacement)
        } else {
            self.displacement[index].map(|d| d + displacement)
        };
    }
    /// Joins `old` and `new` into the caster's union; whether it is new this frame.
    fn union(&mut self, index: usize, old: [Vec3; 2], new: [Vec3; 2]) -> bool {
        let bounds = [old[0].min(new[0]), old[1].max(new[1])];
        if self.casters.insert(index) {
            self.unions[index] = bounds;
            true
        } else {
            self.unions[index][0] = self.unions[index][0].min(bounds[0]);
            self.unions[index][1] = self.unions[index][1].max(bounds[1]);
            false
        }
    }
}

impl Native {
    pub(super) fn update(
        &mut self,
        gpu: &mut GpuDevice,
        scene: &NativeScene,
        changes: Option<&Changes>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.update");
        self.update_gpu_ms = 0.;
        ensure!(
            scene.instances.len() == self.rows.len()
                && scene.materials.len() == self.material_data.len()
                && scene.geometry.len() == self.geometry_revisions.len(),
            "Native raster layout changed"
        );
        // Snapshot callers retain compatibility; extracted live frames already
        // carry exact dirty rows and never scan the foliage suffix here.
        let mut discovered = Changes::default();
        let snapshot = changes.is_none();
        if changes.is_none() {
            self.groups.validate(scene)?;
            for (i, instance) in scene.instances.iter().enumerate() {
                if instance.world != self.rows[i].world {
                    discovered.instances.push(i);
                }
            }
            for (i, material) in scene.materials.iter().enumerate() {
                if bytemuck::bytes_of(material) != bytemuck::bytes_of(&self.material_data[i]) {
                    discovered.materials.push(i);
                }
            }
            for (i, geometry) in scene.geometry.iter().enumerate() {
                if geometry.revision != self.geometry_revisions[i] {
                    discovered.geometries.push(i);
                }
            }
            for (i, instance) in scene.instances.iter().enumerate() {
                if let (Some(next), Some(known)) = (&instance.paint, &self.paint_sources[i])
                    && !std::sync::Arc::ptr_eq(next, known)
                    && next != known
                {
                    discovered.paint.push(i);
                }
            }
        }
        let changes = changes.unwrap_or(&discovered);
        // Validate every candidate before touching any published row.
        for &i in &changes.instances {
            let instance = scene
                .instances
                .get(i)
                .context("Invalid native instance row")?;
            self.groups.validate_instance(i, instance)?;
            ensure!(
                instance.world.is_finite() && instance.world.determinant().abs() > 1e-12,
                "Invalid native instance transform"
            );
        }
        for &i in &changes.materials {
            let material = scene
                .materials
                .get(i)
                .context("Invalid native material row")?;
            ensure!(
                material
                    .pbr
                    .base_color
                    .iter()
                    .chain(&material.pbr.emissive_roughness)
                    .chain(&material.pbr.surface)
                    .all(|v| v.is_finite()),
                "Non-finite native material"
            );
        }
        for &i in &changes.geometries {
            ensure!(i < scene.geometry.len(), "Invalid native geometry row");
        }
        if changes.decals {
            scene.validate_decals()?;
        }
        for &i in &changes.paint {
            let instance = scene.instances.get(i).context("Invalid native paint row")?;
            self.groups.validate_instance(i, instance)?;
            ensure!(
                instance.paint.as_ref().is_some_and(|paint| {
                    paint.len() == scene.geometry[instance.geometry].vertices.len()
                }),
                "Native paint must keep one colour per vertex"
            );
        }
        if snapshot || !changes.geometries.is_empty() {
            if let Some(pose) = self.pool.prepare_pose(gpu, scene)? {
                self.update_gpu_ms = pose.milliseconds;
                self.pool.commit_pose(gpu, pose)?;
                gpu.collect()?;
            }
            for &geometry in &changes.geometries {
                self.geometry_revisions[geometry] = scene.geometry[geometry].revision;
                for n in 0..self.groups.geometry_instances[geometry].len() {
                    let i = self.groups.geometry_instances[geometry][n];
                    self.changes.skins.insert(i);
                    self.update_bounds(i, scene, None);
                }
            }
        }
        for &i in &changes.materials {
            let material = scene.materials[i];
            if bytemuck::bytes_of(&material) == bytemuck::bytes_of(&self.material_data[i]) {
                continue;
            }
            gpu.upload_queued_range(
                self.materials,
                (i * std::mem::size_of_val(&material)) as u64,
                bytemuck::bytes_of(&material),
            )?;
            if batches::is_translucent(&material) || batches::is_translucent(&self.material_data[i])
            {
                self.translucent_revision = self.translucent_revision.wrapping_add(1);
            }
            self.material_data[i] = material;
            self.material_dirty = true;
            self.changes.materials += 1;
            self.culling.material(&mut self.groups, i, &material);
            for &instance in &self.groups.material_instances[i] {
                self.changes
                    .caster(instance, self.bounds[instance], self.bounds[instance]);
            }
        }
        for &i in &changes.instances {
            if self.rows[i].world == scene.instances[i].world {
                continue;
            }
            let old = self.rows[i].world;
            self.rows[i] = row(
                &scene.instances[i],
                self.presented[i],
                self.paint_index,
                self.paint_offsets[i],
            );
            self.changes.rows.insert(i);
            self.update_bounds(i, scene, Some(old));
        }
        for &i in &changes.paint {
            let paint = scene.instances[i]
                .paint
                .as_ref()
                .expect("validated paint rows carry paint");
            self.paint_sources[i] = Some(paint.clone());
            gpu.upload_queued_range(
                self.paint,
                u64::from(self.paint_offsets[i]) * 4,
                bytemuck::cast_slice(paint),
            )?;
            use sha2::Digest;
            let mut digest = sha2::Sha256::new();
            digest.update(self.paint_digest);
            digest.update((i as u64).to_le_bytes());
            digest.update(bytemuck::cast_slice::<u32, u8>(paint));
            self.paint_digest = digest.finalize().into();
            self.paint_dirty = true;
            // Masked casters' alpha test reads the colours.
            if self.groups.groups[self.groups.instance_group[i]].paint_shadow {
                self.changes.caster(i, self.bounds[i], self.bounds[i]);
            }
        }
        // Snapshot callers report no rows: their decals rewrite (a few records).
        if changes.decals || (snapshot && !scene.decals.is_empty()) {
            scene.validate_decals()?;
            self.decals.update(gpu, &scene.decals)?;
        }
        if !changes.instances.is_empty()
            || !changes.materials.is_empty()
            || !changes.geometries.is_empty()
            || !changes.paint.is_empty()
        {
            self.view_revision = self.view_revision.wrapping_add(1);
        }
        Ok(())
    }

    /// `moved`: the row's previous transform when only its transform changed (a rigid
    /// move, whose displacement bound the sun page service reads).
    fn update_bounds(&mut self, i: usize, scene: &NativeScene, moved: Option<Mat4>) {
        let instance = &scene.instances[i];
        let next = instance.bounds(&scene.geometry[instance.geometry]);
        match moved {
            Some(old) => {
                let d = rigid_displacement(old, instance.world, next);
                self.changes.caster_moved(i, self.bounds[i], next, d);
            }
            None => self.changes.caster(i, self.bounds[i], next),
        }
        self.bounds[i] = next;
        self.culling.instance(&self.groups, i, instance.world, next);
        if self.groups.groups[self.groups.instance_group[i]].translucent {
            self.translucent_revision = self.translucent_revision.wrapping_add(1);
        }
    }

    pub(in crate::renderer) fn prepare(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &frame::Frame,
        camera: &crate::scene::Camera,
        quality: &crate::look::Quality,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.prepare");
        self.changes.work = FrameWork {
            material_rows: self.changes.materials,
            ..Default::default()
        };
        self.upload = self.textures.refine(gpu)?;
        // Motion needs exactly one final write when an actor stops. Settled
        // instances are not serialized or visited again on idle frames.
        for i in self.changes.settle.drain(..) {
            self.rows[i].previous = self.presented[i];
            self.rows[i].meta[1] &= !1;
            self.changes.rows.insert(i);
        }
        for &i in &self.changes.skins.list {
            self.rows[i].meta[1] |= 1;
            self.changes.rows.insert(i);
        }
        self.changes.rows.list.sort_unstable();
        for &i in &self.changes.rows.list {
            self.rows[i].previous = self.presented[i];
        }
        let vp = camera.projection * camera.view;
        let header = [vp, self.previous_vp.unwrap_or(vp)];
        if self.changes.header != Some(header) {
            gpu.upload_queued_range(self.instances, 0, bytemuck::cast_slice(&header))?;
            self.changes.header = Some(header);
        }
        let dirty = &self.changes.rows.list;
        let mut start = 0;
        while start < dirty.len() {
            let mut end = start + 1;
            while end < dirty.len() && dirty[end] == dirty[end - 1] + 1 {
                end += 1;
            }
            let first = dirty[start];
            let last = dirty[end - 1] + 1;
            gpu.upload_queued_range(
                self.instances,
                (128 + first * std::mem::size_of::<InstanceRow>()) as u64,
                bytemuck::cast_slice(&self.rows[first..last]),
            )?;
            self.changes.work.instance_rows += last - first;
            start = end;
        }
        let budgets_changed = self.culling.configure(quality);
        self.changes.caster_all = self.changes.all || budgets_changed;
        self.changes.caster_bounds.clear();
        self.changes.caster_rows.clear();
        self.changes.caster_displacement.clear();
        for &i in &self.changes.casters.list {
            self.changes.caster_bounds.push(self.changes.unions[i]);
            self.changes.caster_rows.push(i);
            self.changes
                .caster_displacement
                .push(self.changes.displacement[i]);
        }
        self.culling.prepare(
            gpu,
            self.visible,
            &self.groups,
            &self.bounds,
            frame,
            camera,
            quality.shadows.resolution,
            &mut self.changes.work,
        )?;
        Ok(())
    }

    pub(in crate::renderer) fn presented(
        &mut self,
        camera: &crate::scene::Camera,
        advance_geometry: bool,
    ) {
        for &i in &self.changes.rows.list {
            let row = &self.rows[i];
            self.presented[i] = row.world;
            if row.previous != row.world || row.meta[1] & 1 != 0 {
                self.changes.settle.push(i);
            }
        }
        self.changes.rows.clear();
        self.changes.skins.clear();
        self.changes.casters.clear();
        self.changes.all = false;
        self.changes.materials = 0;
        self.previous_vp = Some(camera.projection * camera.view);
        self.material_dirty = false;
        self.paint_dirty = false;
        self.decals.dirty = false;
        if advance_geometry {
            self.pool.presented();
        }
    }
}

impl Native {
    /// Instance rows rewritten this frame and instances whose skin pose changed
    /// (`renderer::rt` refreshes their TLAS transforms and skin BLASes from these);
    /// cleared once a frame is presented.
    pub(in crate::renderer) fn ray_changes(&self) -> (&[usize], &[usize]) {
        (&self.changes.rows.list, &self.changes.skins.list)
    }
}

/// The largest distance any point of world box `bounds` (at transform `new`) moved
/// since transform `old`: `(new - old) new^-1` is affine, so its largest length over
/// the box is at a corner. The sun page service's dynamic-layer bound.
fn rigid_displacement(old: Mat4, new: Mat4, [lo, hi]: [Vec3; 2]) -> f32 {
    let delta = (new - old) * new.inverse();
    (0..8u32)
        .map(|c| {
            let p = Vec3::new(
                if c & 1 == 0 { lo.x } else { hi.x },
                if c & 2 == 0 { lo.y } else { hi.y },
                if c & 4 == 0 { lo.z } else { hi.z },
            );
            (delta * p.extend(1.)).truncate().length()
        })
        .fold(0., f32::max)
}
