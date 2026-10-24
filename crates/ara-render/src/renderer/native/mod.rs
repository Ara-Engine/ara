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
#[cfg(test)]
mod tests;
use batches::{Batch, Groups};

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
    draws: [Vec<Batch>; 6],
    visible_rows: Vec<[u32; 2]>,
    transparent: Vec<(f32, usize, u32)>,
    staging: Vec<u8>,
    pipelines: [Pipeline; 3],
    upload: TextureStats,
    material_dirty: bool,
    pose_changed: bool,
}

impl Renderer {
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
        let environment = Scene {
            meshes: Vec::new(),
            materials: vec![crate::scene::Material {
                textures: [0; 4],
                ..Default::default()
            }],
            textures: vec![crate::scene::Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            }],
            lights: scene.lights.to_vec(),
            sun_direction: scene.sun_direction,
            sun_lux: scene.sun_lux,
            environment: scene.environment.to_vec(),
            environment_size: scene.environment_size,
        };
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
        native.update(&mut self.gpu, scene)?;
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

    /// Compressed texture upload costs of the last completed native frame.
    pub fn native_texture_stats(&self) -> Option<TextureStats> {
        self.native.as_ref().map(|native| native.upload)
    }

    /// Shared geometry payload and optional-AS counts for the native generation.
    pub fn native_geometry_stats(&self) -> Option<crate::instanced::GeometryUpload> {
        self.native.as_ref().map(|native| native.pool.stats())
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
            let visible = host_buffer(gpu, (scene.instances.len() * 6 * 8).max(16), &mut owned)?;
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
        let result = Self {
            material_index,
            instance_index,
            visible_index,
            groups: Groups::new(scene, &pool)?,
            pool,
            textures,
            texture_digest,
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
            draws: std::array::from_fn(|_| Vec::new()),
            visible_rows: Vec::with_capacity(scene.instances.len() * 6),
            transparent: Vec::with_capacity(scene.instances.len()),
            staging: Vec::with_capacity(
                128 + scene.instances.len() * std::mem::size_of::<InstanceRow>(),
            ),
            pipelines: pipelines
                .try_into()
                .map_err(|_| anyhow::anyhow!("Native raster pipeline count"))?,
            upload,
            material_dirty: true,
            pose_changed: false,
        };
        Ok(result)
    }

    fn update(&mut self, gpu: &mut VulkanDevice, scene: &NativeScene) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.update");
        ensure!(
            scene.instances.len() == self.rows.len()
                && scene.materials.len() == self.material_data.len(),
            "Native raster layout changed"
        );
        self.groups.validate(scene)?;
        ensure!(
            scene
                .instances
                .iter()
                .all(|instance| instance.world.is_finite()
                    && instance.world.determinant().abs() > 1e-12),
            "Invalid native instance transform"
        );
        ensure!(
            scene.materials.iter().all(|material| material
                .pbr
                .base_color
                .iter()
                .chain(&material.pbr.emissive_roughness)
                .chain(&material.pbr.surface)
                .all(|value| value.is_finite())),
            "Non-finite native material"
        );
        // Pose preparation never builds BLASes when GeometryGpu was created for raster.
        if let Some(pose) = self.pool.prepare_pose(gpu, scene)? {
            self.pool.commit_pose(gpu, pose)?;
            self.pose_changed = true;
            gpu.collect()?;
        }
        if bytemuck::cast_slice::<_, u8>(&self.material_data)
            != bytemuck::cast_slice::<_, u8>(&scene.materials)
        {
            self.material_data.copy_from_slice(&scene.materials);
            gpu.upload(self.materials, bytemuck::cast_slice(&self.material_data))?;
            self.material_dirty = true;
        }
        for (index, instance) in scene.instances.iter().enumerate() {
            self.rows[index] = row(instance, self.presented[index]);
            self.bounds[index] = instance.bounds(&scene.geometry[instance.geometry]);
        }
        Ok(())
    }

    pub(super) fn prepare(
        &mut self,
        gpu: &mut VulkanDevice,
        frame: &frame::Frame,
        camera: &crate::scene::Camera,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.prepare");
        self.upload = self.textures.refine(gpu)?;
        for (row, previous) in self.rows.iter_mut().zip(&self.presented) {
            row.previous = *previous;
            row.meta[1] = u32::from(self.pose_changed);
        }
        let vp = camera.projection * camera.view;
        self.staging.clear();
        self.staging.extend_from_slice(bytemuck::bytes_of(&vp));
        self.staging
            .extend_from_slice(bytemuck::bytes_of(&self.previous_vp.unwrap_or(vp)));
        self.staging
            .extend_from_slice(bytemuck::cast_slice(&self.rows));
        gpu.upload(self.instances, &self.staging)?;
        self.prepare_batches(frame, camera);
        if !self.visible_rows.is_empty() {
            gpu.upload(self.visible, bytemuck::cast_slice(&self.visible_rows))?;
        }
        Ok(())
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
            self.pipelines[if pass < 4 {
                0
            } else if pass == 4 {
                1
            } else {
                2
            }],
        ));
        for batch in &self.draws[pass] {
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

    pub(super) fn presented(&mut self, camera: &crate::scene::Camera) {
        for (previous, row) in self.presented.iter_mut().zip(&self.rows) {
            *previous = row.world;
        }
        self.previous_vp = Some(camera.projection * camera.view);
        self.material_dirty = false;
        self.pose_changed = false;
        self.pool.presented();
    }

    pub(super) fn hash(&self, hash: &mut sha2::Sha256) {
        use sha2::Digest;
        hash.update(bytemuck::cast_slice(&self.rows));
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
