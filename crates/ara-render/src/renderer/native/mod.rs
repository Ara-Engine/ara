//! Native raster content: local geometry, compressed residency and draw instances.
use super::*;
use crate::instanced::{GeometryGpu, Scene as NativeScene, TextureStats, Textures};
use ara_gpu::{
    RootConstants,
    commands::{Command, Recorder},
};
use bytemuck::{Pod, Zeroable};
use glam::Vec4;

mod batches;
mod culling;
mod environment;
#[cfg(test)]
mod tests;
mod updates;
use batches::Groups;
use culling::Culling;

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
    materials: Buffer,
    material_index: u32,
    material_data: Vec<crate::instanced::Material>,
    instances: Buffer,
    instance_index: u32,
    visible: Buffer,
    visible_index: u32,
    rows: Vec<InstanceRow>,
    presented: Vec<Mat4>,
    previous_vp: Option<Mat4>,
    bounds: Vec<[Vec3; 2]>,
    groups: Groups,
    culling: Culling,
    changes: updates::Updates,
    pipelines: [Pipeline; 3],
    upload: TextureStats,
    material_dirty: bool,
    update_gpu_ms: f64,
}

impl Renderer {
    /// Whether this view draws native local geometry and compressed materials.
    pub fn is_native(&self) -> bool {
        self.native.is_some()
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
        scene.validate()?;
        let environment = environment::snapshot(scene);
        let mut renderer = Self::new_device(environment, size, extensions, cache.into(), true)?;
        renderer.replace_native_scene(scene, textures)?;
        Ok(renderer)
    }

    /// Publish a complete native content generation. Preparation failure keeps the
    /// previous geometry, textures and presentation; moving instances use `update_native`.
    pub fn replace_native_scene(
        &mut self,
        scene: &NativeScene,
        textures: Box<dyn Textures>,
    ) -> Result<()> {
        let next = Native::new(&mut self.gpu, scene, textures, &self.artifacts)?;
        // The first native generation replaces bootstrap/legacy geometry. HDRI
        // changes rebuild the actual environment and its IBL, rather than merely
        // changing the capture metadata. Ordinary motion never enters this path.
        let environment = environment::snapshot(scene);
        if !self.content.meshes.is_empty()
            || super::hash::static_hash(&environment) != self.content.static_hash
        {
            let content = match upload::upload_content(
                &mut self.gpu,
                environment,
                &self.artifacts,
                &self.shader_metadata,
                self.compute_frame,
            ) {
                Ok(content) => content,
                Err(error) => {
                    next.destroy(&mut self.gpu)?;
                    return Err(error);
                }
            };
            let previous = std::mem::replace(&mut self.content, content);
            self.retire_content(previous)?;
        }
        let mut hash = sha2::Sha256::new();
        use sha2::Digest;
        hash.update(self.content.static_hash.as_bytes());
        for geometry in &scene.geometry {
            hash.update(geometry.key);
        }
        self.content.base_hash = format!("{:x}", hash.finalize());
        let names = scene
            .instances
            .iter()
            .map(|i| (i.entity, i.name.to_owned()))
            .collect();
        let old = self.native.replace(next);
        self.content.entity_names = names;
        self.content.revision += 1;
        self.targets.history = None;
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
                self.gpu.upload(buffer, bytemuck::cast_slice(lights))?;
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
    /// matrix, using the main camera's selected LODs. Call after native preparation
    /// and before declaring the shadow pass resources; record it as native pass 6.
    pub fn cull_native_shadow(&mut self, matrix: Mat4) -> Result<()> {
        let native = self
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Native raster content required"))?;
        native.culling.shadow(
            &self.gpu,
            native.visible,
            &native.groups,
            &native.bounds,
            matrix,
            &mut native.changes.work,
        )
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
    fn new(
        gpu: &mut VulkanDevice,
        scene: &NativeScene,
        mut textures: Box<dyn Textures>,
        artifacts: &Path,
    ) -> Result<Self> {
        scene.validate()?;
        let texture_digest = textures.content_digest();
        let upload = match textures.initialize(gpu) {
            Ok(upload) => upload,
            Err(error) => {
                textures.destroy(gpu)?;
                return Err(error);
            }
        };
        if textures.binding().map_or(0, |b| b.textures) != scene.textures {
            textures.destroy(gpu)?;
            anyhow::bail!("Native texture-table count mismatch");
        }
        let pool = match GeometryGpu::new(gpu, scene, false) {
            Ok(pool) => pool,
            Err(error) => {
                textures.destroy(gpu)?;
                return Err(error);
            }
        };
        let mut owned = Vec::new();
        let mut pipelines = Vec::new();
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
            let visible = host_buffer(gpu, (scene.instances.len() * 7 * 8).max(16), &mut owned)?;
            for name in ["native_shadow", "native_gbuffer", "native_forward"] {
                let cooked = serde_json::from_slice(&std::fs::read(
                    artifacts.join(format!("{name}.json")),
                )?)?;
                let pipeline = gpu.create_pipeline(&cooked, artifacts)?;
                pipelines.push(pipeline);
            }
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
                    for pipeline in pipelines {
                        gpu.retire_pipeline(pipeline)?;
                    }
                    gpu.collect()?;
                    return Err(error);
                }
            };
        let rows = scene.instances.iter().map(|i| row(i, i.world)).collect();
        let groups = Groups::new(scene, &pool)?;
        let culling = Culling::new(&groups, scene.instances.len());
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
            materials,
            instances,
            visible,
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
            pipelines: pipelines
                .try_into()
                .map_err(|_| anyhow::anyhow!("Native raster pipeline count"))?,
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
        ara_gpu::cpu_scope!("RasterNative.record");
        recorder.push(Command::Pipeline(
            self.pipelines[if pass < 4 || pass == 6 {
                0
            } else if pass == 4 {
                1
            } else {
                2
            }],
        ));
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

    fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        self.pool.destroy(gpu)?;
        self.textures.destroy(gpu)?;
        for buffer in [self.materials, self.instances, self.visible] {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        for pipeline in self.pipelines {
            gpu.retire_pipeline(pipeline)?;
        }
        gpu.collect()?;
        Ok(())
    }
}

fn host_buffer(gpu: &mut VulkanDevice, size: usize, owned: &mut Vec<Resource>) -> Result<Buffer> {
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
