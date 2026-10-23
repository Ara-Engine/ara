//! Immutable scene upload and incremental, stable-ID acceleration snapshots.
use anyhow::{Result, ensure};
use ara_gpu::{
    AccelerationStructure, Access, Barrier, Buffer, Device, Image, Queue, Resource,
    acceleration::{AccelerationDesc, AccelerationInstance},
    vk,
    vulkan::{Command, VulkanDevice},
};
use ara_render::scene::{Material, Scene};
use glam::Vec3;
use sha2::{Digest, Sha256};

mod acceleration;
mod packing;
mod upload;
use packing::{content_hash, pack, texture_hash, validate};
pub(crate) use upload::{buffer, upload_image};

/// A cached BLAS and the exact input revision used to build it.
#[derive(Clone, Copy)]
struct Geometry {
    entity: u64,
    hash: [u8; 32],
    topology_hash: [u8; 32],
    blas: AccelerationStructure,
    buffers: [Buffer; 2],
}
/// Counts from the last scene update. Native refit is unavailable in the merged
/// device API, so same-topology changes explicitly count their rebuild fallback.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct AccelerationUpdate {
    /// Geometry allocations retained without a build.
    pub reused_blas: usize,
    /// New native BLAS builds.
    pub rebuilt_blas: usize,
    /// Same-topology deformations rebuilt because the backend only supports static AS.
    pub refit_fallbacks: usize,
    /// Whether instance storage was rebuilt; light/material-only updates skip this.
    pub tlas_rebuilt: bool,
}

/// Immutable GPU extraction, shared by hybrid and reference views.
pub struct GpuScene {
    pub(crate) table: Buffer,
    pub(crate) table_index: u32,
    pub(crate) previous_table: Option<(Buffer, u32)>,
    pub(crate) motion_pending: bool,
    texture_indices: Vec<u32>,
    texture_hash: [u8; 32],
    masks: std::collections::BTreeMap<u64, u8>,
    pub(crate) images: Vec<Image>,
    pub(crate) environment: u32,
    pub(crate) tlas: Option<AccelerationStructure>,
    pub(crate) tlas_index: u32,
    pub(crate) bounds: [Vec3; 2],
    pub(crate) sun_direction: Vec3,
    pub(crate) sun_lux: Vec3,
    pub(crate) hash: String,
    geometry: Vec<Geometry>,
    acceleration_update: AccelerationUpdate,
    ids: std::collections::BTreeMap<u64, u32>,
}
impl GpuScene {
    /// Upload shared material/mesh/light semantics; RT is optional for fallback views.
    pub fn new(gpu: &mut VulkanDevice, scene: &Scene, rt: bool) -> Result<Self> {
        validate(scene)?;
        let ids = scene
            .meshes
            .iter()
            .enumerate()
            .map(|(i, m)| (m.entity, i as u32))
            .collect();
        let mut result = Self {
            table: buffer(gpu, &[0; 16], vk::BufferUsageFlags::STORAGE_BUFFER)?,
            table_index: 0,
            previous_table: None,
            motion_pending: false,
            texture_indices: Vec::new(),
            texture_hash: texture_hash(scene),
            masks: std::collections::BTreeMap::new(),
            images: Vec::new(),
            environment: 0,
            tlas: None,
            tlas_index: 0,
            bounds: [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)],
            sun_direction: scene.sun_direction.normalize(),
            sun_lux: scene.sun_lux,
            hash: String::new(),
            geometry: Vec::new(),
            acceleration_update: AccelerationUpdate::default(),
            ids,
        };
        let mut materials = scene.materials.to_vec();
        let mut texture_indices = Vec::new();
        for texture in &scene.textures {
            let format = if texture.srgb {
                vk::Format::R8G8B8A8_SRGB
            } else {
                vk::Format::R8G8B8A8_UNORM
            };
            let image = upload_image(gpu, texture.size, format, &texture.rgba)?;
            texture_indices.push(gpu.sampled_image(image, false)?.index);
            result.images.push(image);
        }
        for m in &mut materials {
            for t in &mut m.textures {
                *t = texture_indices[*t as usize];
            }
        }
        result.texture_indices = texture_indices;
        let env = upload_image(
            gpu,
            scene.environment_size,
            vk::Format::R32G32B32A32_SFLOAT,
            bytemuck::cast_slice(&scene.environment),
        )?;
        result.environment = gpu.sampled_image(env, false)?.index;
        result.images.push(env);
        let bytes = pack(scene, &materials, &result.ids, &mut result.bounds);
        gpu.retire(Resource::Buffer(result.table))?;
        result.table = buffer(gpu, &bytes, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        result.table_index = gpu.storage_buffer(result.table)?.index;
        result.hash = content_hash(scene, &result.ids, &mut result.bounds);
        if rt {
            result.update_acceleration(gpu, scene)?;
        }
        gpu.collect()?;
        Ok(result)
    }
    /// Apply a validated extraction at a completed frame boundary. Texture/environment
    /// replacement needs a new view; geometry, material parameters and lights update here.
    /// Shading tables retain the prior vertex positions for deformation motion vectors.
    pub fn update(&mut self, gpu: &mut VulkanDevice, scene: &Scene) -> Result<()> {
        validate(scene)?;
        ensure!(
            texture_hash(scene) == self.texture_hash,
            "Texture/environment changes require a replacement view"
        );
        for mesh in &scene.meshes {
            if !self.ids.contains_key(&mesh.entity) {
                ensure!(self.ids.len() < 4096, "Stable instance table exhausted");
                self.ids.insert(mesh.entity, self.ids.len() as u32);
            }
        }
        let mut materials = scene.materials.to_vec();
        for m in &mut materials {
            for t in &mut m.textures {
                *t = self.texture_indices[*t as usize];
            }
        }
        let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
        let bytes = pack(scene, &materials, &self.ids, &mut bounds);
        let table = buffer(gpu, &bytes, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let index = gpu.storage_buffer(table)?.index;
        if self.tlas.is_some() {
            self.update_acceleration(gpu, scene)?;
        }
        if let Some((old, _)) = self.previous_table.take() {
            gpu.retire(Resource::Buffer(old))?;
        }
        self.previous_table = Some((std::mem::replace(&mut self.table, table), self.table_index));
        self.table_index = index;
        self.motion_pending = true;
        self.sun_direction = scene.sun_direction.normalize();
        self.sun_lux = scene.sun_lux;
        self.hash = content_hash(scene, &self.ids, &mut bounds);
        self.bounds = bounds;
        gpu.collect()?;
        Ok(())
    }
    /// Last update's explicit reuse/rebuild/refit-fallback policy results.
    pub fn acceleration_update(&self) -> AccelerationUpdate {
        self.acceleration_update
    }
    /// Stable shader ID for an external entity; IDs are never compacted during a view.
    pub fn instance_id(&self, entity: u64) -> Option<u32> {
        self.ids.get(&entity).copied()
    }
    /// Content fingerprint used by baked probes and capture provenance.
    pub fn content_hash(&self) -> &str {
        &self.hash
    }
    pub(crate) fn resources(&self) -> Vec<(Resource, Access)> {
        let mut r = vec![(Resource::Buffer(self.table), Access::ShaderRead)];
        if let Some((previous, _)) = self.previous_table {
            r.push((Resource::Buffer(previous), Access::ShaderRead));
        }
        r.extend(
            self.images
                .iter()
                .map(|i| (Resource::Image(*i), Access::ShaderRead)),
        );
        if let Some(t) = self.tlas {
            r.push((Resource::AccelerationStructure(t), Access::AsRead));
        }
        r
    }
    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(t) = self.tlas {
            gpu.retire(Resource::AccelerationStructure(t))?;
        }
        for g in self.geometry {
            gpu.retire(Resource::AccelerationStructure(g.blas))?;
            for b in g.buffers {
                gpu.retire(Resource::Buffer(b))?;
            }
        }
        gpu.retire(Resource::Buffer(self.table))?;
        if let Some((previous, _)) = self.previous_table {
            gpu.retire(Resource::Buffer(previous))?;
        }
        for i in self.images {
            gpu.retire(Resource::Image(i))?;
        }
        Ok(())
    }
}
