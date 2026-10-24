//! Dynamic snapshots retain immutable source pools and only publish after success.
use super::*;
use ara_render::instanced::Material as NativeMaterial;

/// Changes published by a native frame update.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeUpdate {
    /// A pose, surface or light parameter changed.
    pub changed: bool,
    /// Surface/lighting response should reject stale accumulated radiance.
    pub lighting: bool,
    /// Objects differ from their most recently presented poses.
    pub motion: bool,
    /// GPU time for pose upload/BLAS and shading-table/TLAS preparation. Add this
    /// to the subsequent view timings when measuring the complete frame budget.
    pub gpu_ms: f64,
}

struct Change {
    table: bool,
    tlas: bool,
    motion: bool,
    emitters: bool,
}

pub(super) struct Motion {
    geometry: Vec<([u8; 32], u64)>,
    presented_geometry: Vec<u64>,
    ids: Vec<(u64, usize)>,
    bindings: Vec<usize>,
    pub(super) current: Vec<glam::Mat4>,
    pub(super) presented: Vec<glam::Mat4>,
    materials: Vec<NativeMaterial>,
    lights: Vec<ara_render::scene::Light>,
    base: [u8; 32],
}

impl Motion {
    pub(super) fn new(scene: &NativeScene, textures: &NativeResidency) -> Self {
        let mut base = Sha256::new();
        for geometry in &scene.geometry {
            base.update(geometry.key);
            if geometry.deforming {
                base.update(bytemuck::cast_slice(&geometry.vertices));
            }
        }
        base.update(bytemuck::cast_slice(&scene.environment_size));
        base.update(bytemuck::cast_slice(&scene.environment));
        for (key, srgb) in textures.content_keys() {
            base.update(key);
            base.update([u8::from(srgb)]);
        }
        Self {
            geometry: scene.geometry.iter().map(|g| (g.key, g.revision)).collect(),
            presented_geometry: scene.geometry.iter().map(|g| g.revision).collect(),
            ids: scene
                .instances
                .iter()
                .map(|i| (i.entity, i.geometry))
                .collect(),
            bindings: scene.instances.iter().map(|i| i.material).collect(),
            current: scene.instances.iter().map(|i| i.world).collect(),
            presented: scene.instances.iter().map(|i| i.world).collect(),
            materials: scene.materials.to_vec(),
            lights: scene.lights.to_vec(),
            base: base.finalize().into(),
        }
    }

    pub(super) fn fingerprint(&self, scene: &NativeScene) -> String {
        let mut hash = Sha256::new();
        hash.update(self.base);
        for geometry in &scene.geometry {
            hash.update(geometry.revision.to_le_bytes());
        }
        for instance in &scene.instances {
            hash.update(instance.entity.to_le_bytes());
            hash.update(instance.geometry.to_le_bytes());
            hash.update(instance.material.to_le_bytes());
            hash.update(bytemuck::bytes_of(&instance.world));
        }
        hash.update(bytemuck::cast_slice(&scene.materials));
        hash.update(bytemuck::cast_slice(&scene.lights));
        hash.update(bytemuck::bytes_of(&scene.sun_direction));
        hash.update(bytemuck::bytes_of(&scene.sun_lux));
        format!("{:x}", hash.finalize())
    }

    fn check(&self, scene: &NativeScene) -> Result<Change> {
        ensure!(
            self.geometry.len() == scene.geometry.len()
                && self
                    .geometry
                    .iter()
                    .zip(&scene.geometry)
                    .all(|(old, new)| old.0 == new.key && (old.1 == new.revision || new.deforming)),
            "native geometry generation changed; prepare replacement content"
        );
        ensure!(
            self.ids.len() == scene.instances.len(),
            "native instance layout changed; prepare replacement content"
        );
        scene.validate_parameters()?;
        let deformed = self
            .geometry
            .iter()
            .zip(&scene.geometry)
            .any(|(old, new)| old.1 != new.revision);
        let mut table = deformed;
        let mut tlas = deformed;
        let mut motion = self
            .presented_geometry
            .iter()
            .zip(&scene.geometry)
            .any(|(old, new)| *old != new.revision);
        let mut emitters = false;
        for (index, instance) in scene.instances.iter().enumerate() {
            ensure!(
                self.ids[index] == (instance.entity, instance.geometry)
                    && instance.material < scene.materials.len(),
                "native instance identity/material mismatch"
            );
            let world = instance.world;
            ensure!(
                world.is_finite()
                    && world.x_axis.w.abs() < 1e-6
                    && world.y_axis.w.abs() < 1e-6
                    && world.z_axis.w.abs() < 1e-6
                    && (world.w_axis.w - 1.0).abs() < 1e-6
                    && world.determinant().abs() > 1e-12,
                "invalid native instance pose"
            );
            let moved = world != self.current[index];
            let binding = instance.material != self.bindings[index];
            let alpha = (self.materials[self.bindings[index]].pbr.flags[0] == 0)
                != (scene.materials[instance.material].pbr.flags[0] == 0);
            table |= moved || binding;
            tlas |= moved || alpha;
            motion |= world != self.presented[index];
            emitters |= (moved
                || self.geometry[instance.geometry].1
                    != scene.geometry[instance.geometry].revision)
                && (self.materials[self.bindings[index]].pbr.emissive_roughness[..3]
                    .iter()
                    .chain(&scene.materials[instance.material].pbr.emissive_roughness[..3])
                    .any(|v| *v > 0.0));
        }
        Ok(Change {
            table,
            tlas,
            motion,
            emitters,
        })
    }

    fn accept(&mut self, scene: &NativeScene) {
        for (old, new) in self.geometry.iter_mut().zip(&scene.geometry) {
            old.1 = new.revision;
        }
        for (index, instance) in scene.instances.iter().enumerate() {
            self.current[index] = instance.world;
            self.bindings[index] = instance.material;
        }
        self.materials.clear();
        self.materials.extend_from_slice(&scene.materials);
        self.lights.clear();
        self.lights.extend_from_slice(&scene.lights);
    }

    pub(super) fn present_geometry(&mut self) {
        for (presented, current) in self.presented_geometry.iter_mut().zip(&self.geometry) {
            *presented = current.1;
        }
    }
}

impl GpuScene {
    /// Publishes native motion/parameters without touching static source vertices,
    /// texture pixels, static BLASes or source content hashes. Failure keeps the
    /// previous scene handles live; the host continues presenting its last output.
    pub fn update_native(
        &mut self,
        gpu: &mut VulkanDevice,
        scene: &NativeScene,
    ) -> Result<NativeUpdate> {
        let native = self
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("not a native scene"))?;
        let Change {
            table: poses,
            tlas: tlas_changed,
            motion,
            emitters,
        } = native.motion.check(scene)?;
        ensure!(
            native.textures.binding().map_or(0, |b| b.textures) == scene.textures,
            "native texture generation changed"
        );
        let materials = bytemuck::cast_slice::<_, u8>(&native.motion.materials)
            != bytemuck::cast_slice::<_, u8>(&scene.materials);
        let lights = bytemuck::cast_slice::<_, u8>(&native.motion.lights)
            != bytemuck::cast_slice::<_, u8>(&scene.lights);
        let sun =
            self.sun_direction != scene.sun_direction.normalize() || self.sun_lux != scene.sun_lux;
        if !poses && !materials && !lights {
            self.motion_pending = motion;
            self.acceleration_update = AccelerationUpdate {
                reused_blas: native.pool.as_ref().map_or(0, |p| p.stats().blas),
                ..Default::default()
            };
            if sun {
                self.sun_direction = scene.sun_direction.normalize();
                self.sun_lux = scene.sun_lux;
                self.hash = native.motion.fingerprint(scene);
            }
            return Ok(NativeUpdate {
                changed: sun,
                lighting: sun,
                motion,
                gpu_ms: 0.0,
            });
        }
        let pool = native
            .pool
            .as_mut()
            .expect("native scene owns its geometry pool");
        let mut pose = pool.prepare_pose(gpu, scene)?;
        let rebuilt_blas = pose.as_ref().map_or(0, |p| p.blas);
        let pose_ms = pose.as_ref().map_or(0.0, |p| p.milliseconds);
        let mut allocated = Vec::new();
        let result = (|| {
            let material_buffer = if materials {
                host_buffer(gpu, bytemuck::cast_slice(&scene.materials), &mut allocated)?
            } else {
                native
                    .materials
                    .ok_or_else(|| anyhow::anyhow!("native material buffer missing"))?
            };
            let material_index = gpu.storage_buffer(material_buffer)?.index;
            let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
            let bytes = packing::pack_motion(
                scene,
                pool,
                material_index,
                native.textures.binding().map_or(0, |b| b.buffer_index),
                &mut bounds,
                Some(&native.motion.presented),
                pose.as_ref(),
            )?;
            let table = host_buffer(gpu, &bytes, &mut allocated)?;
            let table_index = gpu.storage_buffer(table)?.index;
            let next_tlas = if tlas_changed && self.tlas.is_some() {
                let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(
                    packing::instances(scene, pool, pose.as_ref())?,
                ))?;
                allocated.push(Resource::AccelerationStructure(tlas));
                Some(tlas)
            } else {
                None
            };
            let mut commands = gpu.begin(Queue::Graphics);
            commands.push(Command::BeginPass("Scene.Native.Motion".into()));
            commands.barriers([Barrier {
                resource: Resource::Buffer(table),
                before: Access::HostWrite,
                after: Access::ShaderRead,
                handoff: None,
            }]);
            if materials {
                commands.barriers([Barrier {
                    resource: Resource::Buffer(material_buffer),
                    before: Access::HostWrite,
                    after: Access::ShaderRead,
                    handoff: None,
                }]);
            }
            if let Some(tlas) = next_tlas {
                commands.push(Command::BuildAccelerationStructure(tlas));
                commands.barriers([Barrier {
                    resource: Resource::AccelerationStructure(tlas),
                    before: Access::AsBuild,
                    after: Access::AsRead,
                    handoff: None,
                }]);
            }
            commands.push(Command::EndPass);
            let point = gpu.submit(commands.finish(), &[])?;
            let gpu_ms = gpu
                .resolve(point)?
                .iter()
                .map(|t| t.milliseconds)
                .sum::<f64>();
            let tlas_index = next_tlas
                .map(|t| gpu.bind_acceleration_structure(t).map(|i| i.index))
                .transpose()?;
            Ok((
                table,
                table_index,
                material_buffer,
                next_tlas,
                tlas_index,
                bounds,
                gpu_ms,
            ))
        })();
        let (table, index, material_buffer, next_tlas, tlas_index, bounds, gpu_ms) = match result {
            Ok(result) => result,
            Err(error) => {
                for resource in allocated.into_iter().rev() {
                    gpu.retire(resource)?;
                }
                if let Some(pose) = pose.take() {
                    pool.cancel_pose(gpu, pose)?;
                }
                gpu.collect()?;
                return Err(error);
            }
        };
        if let Some(tlas) = next_tlas {
            if let Some(old) = self.tlas.replace(tlas) {
                gpu.retire(Resource::AccelerationStructure(old))?;
            }
            self.tlas_index = tlas_index.expect("new TLAS was bound");
        }
        if let Some(pose) = pose {
            pool.commit_pose(gpu, pose)?;
        }
        gpu.retire(Resource::Buffer(std::mem::replace(&mut self.table, table)))?;
        self.table_index = index;
        if materials && let Some(old) = native.materials.replace(material_buffer) {
            gpu.retire(Resource::Buffer(old))?;
        }
        self.bounds = bounds;
        self.motion_pending = motion;
        self.sun_direction = scene.sun_direction.normalize();
        self.sun_lux = scene.sun_lux;
        self.has_emitters = scene.instances.iter().any(|i| {
            scene.materials[i.material].pbr.emissive_roughness[..3]
                .iter()
                .any(|v| *v > 0.0)
        });
        self.has_layers = scene
            .materials
            .iter()
            .any(|m| crate::material::has_layers(std::slice::from_ref(&m.pbr)));
        self.many_local_lights = scene.lights.len() > 8;
        self.hash = native.motion.fingerprint(scene);
        native.motion.accept(scene);
        self.acceleration_update = AccelerationUpdate {
            reused_blas: pool.stats().blas - rebuilt_blas,
            rebuilt_blas,
            refit_fallbacks: rebuilt_blas,
            tlas_rebuilt: next_tlas.is_some(),
        };
        gpu.collect()?;
        Ok(NativeUpdate {
            changed: true,
            lighting: materials || lights || sun || emitters,
            motion,
            gpu_ms: gpu_ms + pose_ms,
        })
    }
}
