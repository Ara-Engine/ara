//! Instanced shading tables and native BC residency. Legacy captures retain their ABI.
use super::*;
use ara_render::instanced::{GeometryGpu, Scene as NativeScene};
use ara_texture_stream::{NativeResidency, NativeStats};

mod motion;
pub use motion::NativeUpdate;
mod packing;
#[cfg(test)]
mod tests;

pub(super) struct Native {
    pool: Option<GeometryGpu>,
    materials: Option<Buffer>,
    textures: NativeResidency,
    motion: motion::Motion,
}

impl Native {
    pub(super) fn presented(&mut self) {
        self.motion.presented.copy_from_slice(&self.motion.current);
        self.motion.present_geometry();
        if let Some(pool) = &mut self.pool {
            pool.presented();
        }
    }
    pub(super) fn resources(&self) -> impl Iterator<Item = (Resource, Access)> + '_ {
        self.pool
            .iter()
            .flat_map(GeometryGpu::resources)
            .chain(
                self.materials
                    .map(|buffer| (Resource::Buffer(buffer), Access::ShaderRead)),
            )
            .chain(self.textures.resources())
    }
    pub(super) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(pool) = self.pool {
            pool.destroy(gpu)?;
        }
        if let Some(materials) = self.materials {
            gpu.retire(Resource::Buffer(materials))?;
        }
        self.textures.destroy(gpu)
    }
}

impl GpuScene {
    /// Upload local shared geometry, real transformed instances and compressed maps.
    /// The resident set moves into the view, keeping every indirect image/table alive.
    pub fn new_native(
        gpu: &mut VulkanDevice,
        scene: &NativeScene,
        textures: NativeResidency,
        rt: bool,
    ) -> Result<Self> {
        scene.validate()?;
        let motion = motion::Motion::new(scene, &textures);
        let mut native = Native {
            pool: None,
            materials: None,
            textures,
            motion,
        };
        let mut allocated = Vec::new();
        let result = (|| {
            native.textures.initialize(gpu)?;
            ensure!(
                native.textures.binding().map_or(0, |b| b.textures) == scene.textures,
                "native texture-table count mismatch"
            );
            native.pool = Some(GeometryGpu::new(gpu, scene, rt)?);
            let pool = native.pool.as_ref().expect("geometry pool was initialized");
            let materials =
                host_buffer(gpu, bytemuck::cast_slice(&scene.materials), &mut allocated)?;
            let material_index = gpu.storage_buffer(materials)?.index;
            let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
            let bytes = packing::pack(
                scene,
                pool,
                material_index,
                native.textures.binding().map_or(0, |b| b.buffer_index),
                &mut bounds,
            )?;
            let table = host_buffer(gpu, &bytes, &mut allocated)?;
            let table_index = gpu.storage_buffer(table)?.index;
            let env = upload_image(
                gpu,
                scene.environment_size,
                vk::Format::R32G32B32A32_SFLOAT,
                bytemuck::cast_slice(&scene.environment),
            )?;
            allocated.push(Resource::Image(env));
            let environment = gpu.sampled_image(env, false)?.index;
            let tlas = if rt && !scene.instances.is_empty() {
                let instances = packing::instances(scene, pool, None)?;
                let tlas =
                    gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
                allocated.push(Resource::AccelerationStructure(tlas));
                Some(tlas)
            } else {
                None
            };
            let mut commands = gpu.begin(Queue::Graphics);
            commands.barriers([materials, table].map(|buffer| Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::HostWrite,
                after: Access::ShaderRead,
                handoff: None,
            }));
            if let Some(tlas) = tlas {
                commands.push(Command::BuildAccelerationStructure(tlas));
                commands.barriers([Barrier {
                    resource: Resource::AccelerationStructure(tlas),
                    before: Access::AsBuild,
                    after: Access::AsRead,
                    handoff: None,
                }]);
            }
            let point = gpu.submit(commands.finish(), &[])?;
            gpu.resolve(point)?;
            let tlas_index = tlas
                .map(|tlas| gpu.bind_acceleration_structure(tlas).map(|b| b.index))
                .transpose()?
                .unwrap_or(0);
            let ids = scene
                .instances
                .iter()
                .enumerate()
                .map(|(id, instance)| (instance.entity, id as u32))
                .collect();
            Ok((
                Self {
                    native: None,
                    table,
                    table_index,
                    previous_table: None,
                    motion_pending: false,
                    texture_indices: Vec::new(),
                    texture_hash: [0; 32],
                    masks: std::collections::BTreeMap::new(),
                    images: vec![env],
                    environment,
                    tlas,
                    tlas_index,
                    bounds,
                    sun_direction: scene.sun_direction.normalize(),
                    sun_lux: scene.sun_lux,
                    has_emitters: scene.instances.iter().any(|i| {
                        scene.materials[i.material].pbr.emissive_roughness[..3]
                            .iter()
                            .any(|v| *v > 0.0)
                    }),
                    many_local_lights: scene.lights.len() > 8,
                    has_layers: scene
                        .materials
                        .iter()
                        .any(|m| crate::material::has_layers(std::slice::from_ref(&m.pbr))),
                    hash: native.motion.fingerprint(scene),
                    geometry: Vec::new(),
                    acceleration_update: AccelerationUpdate {
                        rebuilt_blas: pool.stats().blas,
                        tlas_rebuilt: tlas.is_some(),
                        ..Default::default()
                    },
                    ids,
                },
                materials,
            ))
        })();
        match result {
            Ok((mut result, materials)) => {
                native.materials = Some(materials);
                result.native = Some(native);
                Ok(result)
            }
            Err(error) => {
                for resource in allocated.into_iter().rev() {
                    gpu.retire(resource)?;
                }
                native.destroy(gpu)?;
                gpu.collect()?;
                Err(error)
            }
        }
    }

    /// Compressed payload and upload timing of the native material resident set.
    pub fn native_texture_stats(&self) -> Option<NativeStats> {
        self.native.as_ref().map(|n| n.textures.stats())
    }
}

fn host_buffer(
    gpu: &mut VulkanDevice,
    bytes: &[u8],
    allocated: &mut Vec<Resource>,
) -> Result<Buffer> {
    // SAFETY: Read-only limits query for this live device's physical-device handle.
    let limit = unsafe {
        gpu.context()
            .instance
            .get_physical_device_properties(gpu.context().physical)
    }
    .limits
    .max_storage_buffer_range;
    ensure!(
        bytes.len().max(16) as u64 <= u64::from(limit),
        "native shading table exceeds the device storage-buffer limit"
    );
    let buffer = gpu.create_buffer(
        bytes.len().max(16) as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        true,
    )?;
    allocated.push(Resource::Buffer(buffer));
    if !bytes.is_empty() {
        gpu.upload(buffer, bytes)?;
    }
    Ok(buffer)
}
