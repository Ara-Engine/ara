//! Stage skin poses without overwriting either published or presented vertices.
use super::*;
use ara_gpu::{
    Queue,
    acceleration::{AccelerationDesc, TriangleRange},
    commands::Command,
    vk,
};
use upload::{allocate, edge};

/// A completed candidate pose. Retire its dependent old TLAS before committing;
/// cancel it when preparing the replacement TLAS or scene table fails.
#[must_use = "commit or cancel prepared GPU poses"]
pub struct PreparedPose {
    owner: Buffer,
    table: (Buffer, u32),
    replacements: Vec<Option<AccelerationStructure>>,
    revisions: Vec<(usize, u64)>,
    slot: usize,
    /// Number of deforming source geometries uploaded.
    pub geometries: usize,
    /// Number of changed-geometry BLAS builds (all cooked LODs).
    pub blas: usize,
    /// GPU skin upload/barrier and BLAS-build time.
    pub milliseconds: f64,
}

impl PreparedPose {
    /// Candidate geometry table, used while preparing the replacement shading table.
    pub fn table_index(&self) -> u32 {
        self.table.1
    }

    /// Candidate BLAS for an entry, retaining the existing structure when unchanged.
    pub fn blas(&self, pool: &GeometryGpu, entry: u32) -> Option<AccelerationStructure> {
        self.replacements
            .get(entry as usize)
            .copied()
            .flatten()
            .or_else(|| pool.blas(entry))
    }
}

impl GeometryGpu {
    /// Stages changed local skin attributes and builds only their BLASes. Static
    /// topology and vertices remain resident. Three pose slots retain the last
    /// presented pose and the published pose even if a candidate is rejected.
    pub fn prepare_pose(
        &mut self,
        gpu: &mut VulkanDevice,
        scene: &Scene,
    ) -> Result<Option<PreparedPose>> {
        ensure!(
            !self.pending_pose,
            "commit or cancel the outstanding native pose first"
        );
        ensure!(
            scene.geometry.len() == self.layout.len(),
            "native geometry layout changed"
        );
        let mut revisions = Vec::new();
        for (index, (old, new)) in self.layout.iter().zip(&scene.geometry).enumerate() {
            ensure!(
                old.key == new.key
                    && old.deforming == new.deforming
                    && old.vertices == new.vertices.len()
                    && self.ranges[index].count as usize == new.lods.len(),
                "native geometry layout changed"
            );
            if old.revision != new.revision {
                ensure!(
                    old.deforming,
                    "static native geometry changed; prepare replacement content"
                );
                validate_pose(new)?;
                revisions.push((index, new.revision));
            }
        }
        if revisions.is_empty() {
            return Ok(None);
        }
        let buffers = self
            .dynamic
            .ok_or_else(|| anyhow::anyhow!("missing native skin pool"))?;
        let slot = (0..buffers.len())
            .find(|i| *i != self.current_slot && *i != self.presented_slot)
            .expect("three pose slots leave one candidate slot");
        // Refill every skin range: a previously rejected candidate must not leave
        // stale attributes in an unchanged actor's portion of the next upload.
        for (geometry, layout) in scene.geometry.iter().zip(&self.layout) {
            if layout.deforming {
                self.staging[layout.first..layout.first + layout.vertices]
                    .copy_from_slice(&geometry.vertices);
            }
        }
        let owner = self.table.expect("populated skin pool has a table").0;
        let mut allocated = Vec::new();
        let result = (|| {
            gpu.upload(buffers[slot], bytemuck::cast_slice(&self.staging))?;
            let pool = self
                .static_pool
                .expect("populated skin pool has static indices");
            let pool_index = gpu.storage_buffer(pool)?.index;
            let current = gpu.storage_buffer(buffers[slot])?.index;
            let previous = gpu.storage_buffer(buffers[self.presented_slot])?.index;
            let rows: Vec<_> = self
                .lod_data
                .iter()
                .map(|lod| {
                    Row([
                        if lod.deforming { current } else { pool_index },
                        lod.vertex_offset as u32,
                        pool_index,
                        lod.index_offset as u32,
                        if lod.deforming { previous } else { pool_index },
                        lod.vertex_offset as u32,
                        lod.vertex_count,
                        lod.index_count,
                    ])
                })
                .collect();
            let table = allocate(
                gpu,
                (rows.len() * std::mem::size_of::<Row>()) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
                &mut allocated,
            )?;
            gpu.upload(table, bytemuck::cast_slice(&rows))?;
            let table_index = gpu.storage_buffer(table)?.index;
            let mut replacements = vec![None; self.structures.len()];
            let mut built = 0;
            if !self.structures.is_empty() {
                for &(geometry, _) in &revisions {
                    let range = self.ranges[geometry];
                    for row in range.first..range.first + range.count {
                        let lod = &self.lod_data[row as usize];
                        let blas = gpu.create_acceleration_structure(
                            &AccelerationDesc::TriangleRange(TriangleRange {
                                vertices: buffers[slot],
                                vertex_offset: lod.vertex_offset,
                                vertex_count: lod.vertex_count,
                                vertex_stride: std::mem::size_of::<Vertex>() as u64,
                                indices: pool,
                                index_offset: lod.index_offset,
                                index_count: lod.index_count,
                            }),
                        )?;
                        allocated.push(Resource::AccelerationStructure(blas));
                        replacements[row as usize] = Some(blas);
                        built += 1;
                    }
                }
            }
            let mut commands = gpu.begin(Queue::Graphics);
            commands.push(Command::BeginPass("Scene.Geometry.Pose".into()));
            commands.barriers([
                edge(
                    Resource::Buffer(buffers[slot]),
                    Access::HostWrite,
                    Access::ShaderRead,
                ),
                edge(
                    Resource::Buffer(table),
                    Access::HostWrite,
                    Access::ShaderRead,
                ),
            ]);
            if built != 0 {
                commands.barriers([edge(
                    Resource::Buffer(buffers[slot]),
                    Access::HostWrite,
                    Access::AsInput,
                )]);
                for structure in replacements.iter().flatten() {
                    commands.push(Command::BuildAccelerationStructure(*structure));
                    commands.barriers([
                        edge(
                            Resource::AccelerationStructure(*structure),
                            Access::AsBuild,
                            Access::AsInput,
                        ),
                        edge(
                            Resource::AccelerationStructure(*structure),
                            Access::AsBuild,
                            Access::AsRead,
                        ),
                    ]);
                }
            }
            commands.push(Command::EndPass);
            let point = gpu.submit(commands.finish(), &[])?;
            let milliseconds = gpu.resolve(point)?.iter().map(|t| t.milliseconds).sum();
            Ok(PreparedPose {
                owner,
                table: (table, table_index),
                replacements,
                geometries: revisions.len(),
                revisions,
                slot,
                blas: built,
                milliseconds,
            })
        })();
        match result {
            Ok(candidate) => {
                self.pending_pose = true;
                Ok(Some(candidate))
            }
            Err(error) => {
                for resource in allocated.into_iter().rev() {
                    gpu.retire(resource)?;
                }
                gpu.collect()?;
                Err(error)
            }
        }
    }

    /// Publishes a completed pose after dependent old TLAS retirement.
    pub fn commit_pose(&mut self, gpu: &mut VulkanDevice, pose: PreparedPose) -> Result<()> {
        ensure!(
            self.table.map(|v| v.0) == Some(pose.owner),
            "stale native pose candidate"
        );
        for (index, replacement) in pose.replacements.into_iter().enumerate() {
            if let Some(blas) = replacement {
                gpu.retire(Resource::AccelerationStructure(std::mem::replace(
                    &mut self.structures[index],
                    blas,
                )))?;
            }
        }
        gpu.retire(Resource::Buffer(pose.owner))?;
        self.table = Some(pose.table);
        self.current_slot = pose.slot;
        self.pending_pose = false;
        for (index, revision) in pose.revisions {
            self.layout[index].revision = revision;
        }
        Ok(())
    }

    /// Retires unpublished resources, preserving current/presented pose buffers.
    pub fn cancel_pose(&mut self, gpu: &mut VulkanDevice, pose: PreparedPose) -> Result<()> {
        ensure!(
            self.table.map(|v| v.0) == Some(pose.owner),
            "stale native pose candidate"
        );
        for blas in pose.replacements.into_iter().flatten() {
            gpu.retire(Resource::AccelerationStructure(blas))?;
        }
        gpu.retire(Resource::Buffer(pose.table.0))?;
        self.pending_pose = false;
        gpu.collect()?;
        Ok(())
    }

    /// Called only after a successful render; this slot supplies future velocities.
    pub fn presented(&mut self) {
        self.presented_slot = self.current_slot;
    }
}

fn validate_pose(geometry: &super::super::Geometry) -> Result<()> {
    ensure!(
        geometry.bounds.iter().all(|v| v.is_finite())
            && geometry.bounds[0].cmple(geometry.bounds[1]).all(),
        "invalid native skin bounds"
    );
    for vertex in &geometry.vertices {
        let position = glam::Vec3::from_array(vertex.position);
        ensure!(
            position.is_finite()
                && position
                    .cmpge(geometry.bounds[0] - glam::Vec3::splat(1e-4))
                    .all()
                && position
                    .cmple(geometry.bounds[1] + glam::Vec3::splat(1e-4))
                    .all()
                && vertex
                    .normal
                    .iter()
                    .chain(&vertex.uv)
                    .chain(&vertex.uv1)
                    .chain(&vertex.tangent)
                    .all(|v| v.is_finite())
                && glam::Vec3::from_array(vertex.normal).length_squared() > 1e-8
                && glam::Vec3::from_slice(&vertex.tangent).length_squared() > 1e-8,
            "invalid native skin attributes"
        );
    }
    Ok(())
}
