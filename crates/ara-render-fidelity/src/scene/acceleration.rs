//! Incremental BLAS caching and transactional TLAS publication.
use super::*;
impl GpuScene {
    /// Rebuild TLAS masks while retaining every BLAS and stable scene ID.
    /// Mask zero hides an instance from all ray queries; nonzero bits are visible.
    pub fn set_instance_masks(
        &mut self,
        gpu: &mut VulkanDevice,
        masks: &[(u64, u8)],
    ) -> Result<()> {
        ensure!(self.tlas.is_some(), "Instance masks require the RT tier");
        for (id, _) in masks {
            ensure!(self.ids.contains_key(id), "Unknown instance mask entity");
        }
        for (id, mask) in masks {
            self.masks.insert(*id, *mask);
        }
        let instances = self
            .geometry
            .iter()
            .map(|g| AccelerationInstance {
                geometry: g.blas,
                transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
                custom_index: self.ids[&g.entity],
                mask: if self.masks.get(&g.entity).copied().unwrap_or(255) == 0 {
                    0
                } else {
                    g.ray_mask
                },
            })
            .collect();
        let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
        let mut c = gpu.begin(Queue::Graphics);
        c.push(Command::BuildAccelerationStructure(tlas));
        c.barriers([Barrier {
            resource: Resource::AccelerationStructure(tlas),
            before: Access::AsBuild,
            after: Access::AsRead,
            handoff: None,
        }]);
        let point = gpu.submit(c.finish(), &[])?;
        gpu.resolve(point)?;
        self.tlas_index = gpu.bind_acceleration_structure(tlas)?.index;
        if let Some(old) = self.tlas.replace(tlas) {
            gpu.retire(Resource::AccelerationStructure(old))?;
        }
        gpu.collect()?;
        Ok(())
    }
    /// Update only changed BLASes, then atomically publish a replacement TLAS after
    /// its build completes. All masks are 0xff in the base scene extraction ABI.
    pub(super) fn update_acceleration(
        &mut self,
        gpu: &mut VulkanDevice,
        scene: &Scene,
    ) -> Result<()> {
        ensure!(gpu.context().info.ray_tracing, "RT tier is unavailable");
        let mut next = Vec::new();
        let mut stats = AccelerationUpdate::default();
        for mesh in &scene.meshes {
            let mut h = Sha256::new();
            for v in &mesh.vertices {
                h.update(bytemuck::cast_slice(&v.position));
            }
            h.update(bytemuck::cast_slice(&mesh.indices));
            let hash: [u8; 32] = h.finalize().into();
            let mut topology = Sha256::new();
            topology.update(mesh.vertices.len().to_le_bytes());
            topology.update(bytemuck::cast_slice(&mesh.indices));
            let topology_hash: [u8; 32] = topology.finalize().into();
            if let Some(at) = self
                .geometry
                .iter()
                .position(|g| g.entity == mesh.entity && g.hash == hash)
            {
                stats.reused_blas += 1;
                let mut geometry = self.geometry[at];
                geometry.ray_mask = if scene.materials[mesh.material].flags[0] == 0 {
                    1
                } else {
                    2
                };
                next.push(geometry);
                continue;
            }
            stats.rebuilt_blas += 1;
            if self
                .geometry
                .iter()
                .any(|g| g.entity == mesh.entity && g.topology_hash == topology_hash)
            {
                stats.refit_fallbacks += 1;
            }
            let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
            let vertices = buffer(gpu, bytemuck::cast_slice(&mesh.vertices), usage)?;
            let indices = buffer(gpu, bytemuck::cast_slice(&mesh.indices), usage)?;
            let blas = gpu.create_acceleration_structure(&AccelerationDesc::Triangles {
                vertices,
                vertex_count: mesh.vertices.len() as u32,
                vertex_stride: 48,
                indices,
                index_count: mesh.indices.len() as u32,
            })?;
            let mut c = gpu.begin(Queue::Graphics);
            for b in [vertices, indices] {
                c.barriers([Barrier {
                    resource: Resource::Buffer(b),
                    before: Access::ShaderRead,
                    after: Access::AsInput,
                    handoff: None,
                }]);
            }
            c.push(Command::BuildAccelerationStructure(blas));
            c.barriers([Barrier {
                resource: Resource::AccelerationStructure(blas),
                before: Access::AsBuild,
                after: Access::AsInput,
                handoff: None,
            }]);
            let point = gpu.submit(c.finish(), &[])?;
            gpu.resolve(point)?;
            next.push(Geometry {
                entity: mesh.entity,
                hash,
                topology_hash,
                blas,
                ray_mask: if scene.materials[mesh.material].flags[0] == 0 {
                    1
                } else {
                    2
                },
                buffers: [vertices, indices],
            });
        }
        if self.tlas.is_some()
            && next.len() == self.geometry.len()
            && next.iter().all(|g| {
                self.geometry.iter().any(|old| {
                    old.entity == g.entity && old.blas == g.blas && old.ray_mask == g.ray_mask
                })
            })
        {
            self.acceleration_update = stats;
            return Ok(());
        }
        stats.tlas_rebuilt = true;
        let instances = next
            .iter()
            .map(|g| AccelerationInstance {
                geometry: g.blas,
                transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
                custom_index: self.ids[&g.entity],
                mask: if self.masks.get(&g.entity).copied().unwrap_or(255) == 0 {
                    0
                } else {
                    g.ray_mask
                },
            })
            .collect();
        let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
        let mut c = gpu.begin(Queue::Graphics);
        c.push(Command::BeginPass("Scene.TLAS.Build".into()));
        c.push(Command::BuildAccelerationStructure(tlas));
        c.barriers([Barrier {
            resource: Resource::AccelerationStructure(tlas),
            before: Access::AsBuild,
            after: Access::AsRead,
            handoff: None,
        }]);
        c.push(Command::EndPass);
        let point = gpu.submit(c.finish(), &[])?;
        gpu.resolve(point)?;
        self.tlas_index = gpu.bind_acceleration_structure(tlas)?.index;
        if let Some(old) = self.tlas.replace(tlas) {
            gpu.retire(Resource::AccelerationStructure(old))?;
        }
        for g in std::mem::replace(&mut self.geometry, next) {
            if self.geometry.iter().any(|live| live.blas == g.blas) {
                continue;
            }
            gpu.retire(Resource::AccelerationStructure(g.blas))?;
            for b in g.buffers {
                gpu.retire(Resource::Buffer(b))?;
            }
        }
        self.acceleration_update = stats;
        gpu.collect()?;
        Ok(())
    }
}
