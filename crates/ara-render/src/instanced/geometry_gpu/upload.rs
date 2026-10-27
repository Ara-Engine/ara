use super::*;
use ara_gpu::{
    Barrier, Queue,
    acceleration::{AccelerationDesc, TriangleRange},
    commands::Command,
    vk,
};

impl GeometryGpu {
    /// Uploads shared geometry and builds every requested BLAS in one submission.
    /// No instance duplicates, texture pixels or CPU-generated mip levels are packed.
    pub fn new(gpu: &mut GpuDevice, scene: &Scene, rt: bool) -> Result<Self> {
        Self::new_validated(gpu, scene.validated()?, rt, None)
    }

    pub(crate) fn new_validated(
        gpu: &mut GpuDevice,
        validated: super::super::ValidatedScene<'_>,
        rt: bool,
        previous: Option<&Self>,
    ) -> Result<Self> {
        ara_gpu::cpu_scope!("NativeGeometry.create");
        let scene = validated.scene();
        ensure!(
            !rt || gpu.info().ray_tracing,
            "native geometry RT tier unavailable"
        );
        let storage_range = gpu.info().max_storage_buffer_range;
        let previous = previous.filter(|_| !rt);
        let reuse = previous.map(Self::preparation_seed);
        let packed = match previous {
            Some(_) => pack::Packed::new_reusing(scene, u64::from(storage_range), reuse.as_ref())?,
            None => pack::Packed::new(scene, u64::from(storage_range))?,
        };
        let layout = scene
            .geometry
            .iter()
            .zip(&packed.ranges)
            .map(|(geometry, range)| {
                let lod = &packed.lods[range.first as usize];
                GeometryLayout {
                    key: geometry.key,
                    revision: geometry.revision,
                    deforming: geometry.deforming,
                    first: lod.vertex_offset as usize / std::mem::size_of::<Vertex>(),
                    vertices: geometry.vertices.len(),
                }
            })
            .collect();
        let mut allocated = Vec::new();
        match Self::upload(
            gpu,
            packed,
            rt,
            previous.and_then(|p| p.static_pool),
            &mut allocated,
            None,
        ) {
            Ok((pool, stage)) => {
                if let Some(stage) = stage {
                    gpu.retire(Resource::Buffer(stage))?;
                }
                gpu.collect()?;
                Ok(Self { layout, ..pool })
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

    pub(super) fn upload(
        gpu: &mut GpuDevice,
        packed: pack::Packed,
        rt: bool,
        resident: Option<Buffer>,
        allocated: &mut Vec<Resource>,
        mut prepared: Option<&mut preparation::Buffers>,
    ) -> Result<(Self, Option<Buffer>)> {
        ara_gpu::cpu_scope!("NativeGeometry.upload");
        let preallocated = prepared.is_some();
        let mut result = Self {
            static_pool: None,
            dynamic: None,
            table: None,
            ranges: packed.ranges,
            structures: Vec::new(),
            layout: Vec::new(),
            lod_data: Vec::new(),
            staging: Vec::new(),
            current_slot: 0,
            presented_slot: 0,
            pending_pose: false,
            pose_host_writes: 0,
            stats: GeometryUpload::default(),
        };
        result.stats.geometries = result.ranges.len();
        result.stats.reused_geometries = packed.reused_geometries;
        if packed.lods.is_empty() {
            return Ok((result, None));
        }
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::INDEX_BUFFER
            | if rt {
                vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
                    | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            } else {
                vk::BufferUsageFlags::empty()
            };
        let pool = preparation::take_or_allocate(
            gpu,
            preparation::STATIC,
            packed.static_size,
            usage | vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC,
            false,
            allocated,
            &mut prepared,
        )?;
        result.static_pool = Some(pool);
        let stage = if packed.staged_bytes == 0 {
            None
        } else {
            let stage = preparation::take_or_allocate(
                gpu,
                preparation::STAGE,
                packed.staged_bytes,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
                allocated,
                &mut prepared,
            )?;
            if !preallocated {
                gpu.upload(stage, &packed.static_bytes)?;
            }
            Some(stage)
        };
        let pool_index = gpu.storage_buffer(pool)?.index;
        let dynamic_bytes = bytemuck::cast_slice(&packed.dynamic);
        let dynamic = if dynamic_bytes.is_empty() {
            None
        } else {
            let mut skin = [None; 3];
            for (slot, buffer) in skin.iter_mut().enumerate() {
                let b = preparation::take_or_allocate(
                    gpu,
                    preparation::SKIN0 + slot,
                    dynamic_bytes.len() as u64,
                    usage,
                    true,
                    allocated,
                    &mut prepared,
                )?;
                if !preallocated {
                    gpu.upload(b, dynamic_bytes)?;
                }
                *buffer = Some(b);
            }
            let [Some(a), Some(b), Some(c)] = skin else {
                unreachable!("three skin buffers")
            };
            Some([a, b, c])
        };
        result.dynamic = dynamic;
        let dynamic_indices = if let Some([a, _, _]) = dynamic {
            [gpu.storage_buffer(a)?.index; 2]
        } else {
            [0; 2]
        };
        let mut rows = Vec::with_capacity(packed.lods.len());
        for lod in &packed.lods {
            let (vertices, current, previous) = if lod.deforming {
                (
                    dynamic.ok_or_else(|| anyhow::anyhow!("missing deforming vertex pool"))?[0],
                    dynamic_indices[0],
                    dynamic_indices[1],
                )
            } else {
                (pool, pool_index, pool_index)
            };
            rows.push(Row([
                current,
                u32::try_from(lod.vertex_offset)?,
                pool_index,
                u32::try_from(lod.index_offset)?,
                previous,
                u32::try_from(lod.vertex_offset)?,
                lod.vertex_count,
                lod.index_count,
            ]));
            if rt {
                let blas = gpu.create_acceleration_structure(&AccelerationDesc::TriangleRange(
                    TriangleRange {
                        vertices,
                        vertex_offset: lod.vertex_offset,
                        vertex_count: lod.vertex_count,
                        vertex_stride: std::mem::size_of::<Vertex>() as u64,
                        indices: pool,
                        index_offset: lod.index_offset,
                        index_count: lod.index_count,
                    },
                ))?;
                allocated.push(Resource::AccelerationStructure(blas));
                result.structures.push(blas);
            }
        }
        let row_bytes = bytemuck::cast_slice(&rows);
        let table = preparation::take_or_allocate(
            gpu,
            preparation::TABLE,
            row_bytes.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
            allocated,
            &mut prepared,
        )?;
        // Prefetched storage can have an earlier submission's conservative
        // retention stamp even before binding; never overwrite it in flight.
        gpu.upload_queued(table, row_bytes)?;
        result.table = Some((table, gpu.storage_buffer(table)?.index));
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Scene.Geometry.Upload".into()));
        if let Some(stage) = stage {
            commands.barriers([edge(
                Resource::Buffer(stage),
                Access::HostWrite,
                Access::CopySource,
            )]);
        }
        let reuses = packed.copies.iter().any(|copy| copy.resident);
        if reuses {
            commands.barriers([edge(
                Resource::Buffer(
                    resident.ok_or_else(|| anyhow::anyhow!("missing resident geometry source"))?,
                ),
                Access::Geometry,
                Access::CopySource,
            )]);
        }
        commands.barriers([edge(
            Resource::Buffer(pool),
            Access::Undefined,
            Access::CopyDestination,
        )]);
        for copy in &packed.copies {
            let source = if copy.resident { resident } else { stage }
                .ok_or_else(|| anyhow::anyhow!("missing native geometry copy source"))?;
            commands.push(Command::CopyBufferRange {
                source,
                source_offset: copy.source,
                destination: pool,
                destination_offset: copy.destination,
                size: copy.size,
            });
        }
        if reuses {
            commands.barriers([edge(
                Resource::Buffer(
                    resident.ok_or_else(|| anyhow::anyhow!("missing resident geometry source"))?,
                ),
                Access::CopySource,
                Access::Geometry,
            )]);
        }
        // Index and shader reads (Geometry) and AS-build reads consume the
        // original transfer write.
        commands.barriers([edge(
            Resource::Buffer(pool),
            Access::CopyDestination,
            Access::Geometry,
        )]);
        if rt {
            commands.barriers([edge(
                Resource::Buffer(pool),
                Access::CopyDestination,
                Access::AsInput,
            )]);
        }
        for buffer in dynamic.into_iter().flatten() {
            commands.barriers([edge(
                Resource::Buffer(buffer),
                Access::HostWrite,
                Access::ShaderRead,
            )]);
            if rt {
                commands.barriers([edge(
                    Resource::Buffer(buffer),
                    Access::HostWrite,
                    Access::AsInput,
                )]);
            }
        }
        commands.barriers([edge(
            Resource::Buffer(table),
            Access::HostWrite,
            Access::ShaderRead,
        )]);
        commands.push(Command::EndPass);
        if rt {
            commands.push(Command::BeginPass("Scene.Geometry.BLAS.Build".into()));
            for &blas in &result.structures {
                commands.push(Command::BuildAccelerationStructure(blas));
                commands.barriers([
                    edge(
                        Resource::AccelerationStructure(blas),
                        Access::AsBuild,
                        Access::AsInput,
                    ),
                    edge(
                        Resource::AccelerationStructure(blas),
                        Access::AsBuild,
                        Access::AsRead,
                    ),
                ]);
            }
            commands.push(Command::EndPass);
        }
        let point = gpu.submit(commands.finish(), &[])?;
        let timings = gpu.resolve(point)?;
        result.stats.bytes =
            packed.static_size + dynamic_bytes.len() as u64 * 3 + row_bytes.len() as u64;
        result.stats.uploaded_bytes =
            packed.staged_bytes + dynamic_bytes.len() as u64 * 3 + row_bytes.len() as u64;
        result.stats.reused_bytes = packed
            .copies
            .iter()
            .filter(|c| c.resident)
            .map(|c| c.size)
            .sum();
        result.stats.blas = result.structures.len();
        for timing in timings {
            if timing.pass == "Scene.Geometry.Upload" {
                result.stats.upload_ms = timing.milliseconds;
            } else if timing.pass == "Scene.Geometry.BLAS.Build" {
                result.stats.build_ms = timing.milliseconds;
            }
        }
        result.lod_data = packed.lods;
        result.staging = packed.dynamic;
        Ok((result, stage))
    }
}

pub(super) fn allocate(
    gpu: &mut GpuDevice,
    bytes: u64,
    usage: vk::BufferUsageFlags,
    host: bool,
    allocated: &mut Vec<Resource>,
) -> Result<Buffer> {
    let buffer = gpu.create_buffer(bytes, usage, host)?;
    allocated.push(Resource::Buffer(buffer));
    Ok(buffer)
}

pub(super) fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
