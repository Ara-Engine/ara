mod compact;
use super::*;
use crate::acceleration::{
    AccelerationDesc, AccelerationInstance, AccelerationOptions, TriangleRange,
};
mod triangles;

#[derive(Clone, Copy)]
enum Geometry {
    Triangles {
        vertices: Buffer,
        vertex_offset: u64,
        vertex_count: u32,
        stride: u64,
        indices: Buffer,
        index_offset: u64,
        index_count: u32,
    },
    Instances {
        buffer: Buffer,
        count: u32,
    },
}
pub(super) struct NativeAcceleration {
    pub handle: vk::AccelerationStructureKHR,
    /// BLAS address is stable for this object's lifetime, including before build.
    address: vk::DeviceAddress,
    pub backing: BufferAllocation,
    pub scratch: Buffer,
    pub dependencies: Vec<AccelerationStructure>,
    pub auxiliary: Vec<Buffer>,
    pub built: bool,
    built_count: u32,
    compacted: bool,
    pub options: AccelerationOptions,
    pub capacity: u32,
    geometry: Geometry,
}
impl VulkanDevice {
    /// Allocate a static BLAS/TLAS and aligned scratch. Build it through a typed command.
    /// Acceleration structures are optional; the raster tier never requires RT support.
    pub fn create_acceleration_structure(
        &mut self,
        desc: &AccelerationDesc,
    ) -> Result<AccelerationStructure> {
        self.create_acceleration_with_options(desc, AccelerationOptions::default())
    }
    /// Allocate an update-enabled BLAS/TLAS with optional instance headroom.
    pub fn create_acceleration_with_options(
        &mut self,
        desc: &AccelerationDesc,
        options: AccelerationOptions,
    ) -> Result<AccelerationStructure> {
        crate::cpu_scope!("GPU.allocate_acceleration_structure");
        if !self.context.info.ray_tracing {
            return Err(GpuError::Contract(
                "RT-tier acceleration structures are unavailable".into(),
            ));
        }
        let initial = match desc {
            AccelerationDesc::Instances(v) => u32::try_from(v.len())
                .map_err(|_| GpuError::Contract("Too many TLAS instances".into()))?,
            _ => 0,
        };
        let capacity = options.instance_capacity.max(initial);
        if options.instance_capacity != 0 && options.instance_capacity < initial {
            return Err(GpuError::Contract(
                "Instance capacity below initial count".into(),
            ));
        }
        let mut dependencies = Vec::new();
        let mut auxiliary = Vec::new();
        let geometry = match desc {
            AccelerationDesc::Triangles {
                vertices,
                vertex_count,
                vertex_stride,
                indices,
                index_count,
            } => self.triangle_input(TriangleRange {
                vertices: *vertices,
                vertex_offset: 0,
                vertex_count: *vertex_count,
                vertex_stride: *vertex_stride,
                indices: *indices,
                index_offset: 0,
                index_count: *index_count,
            })?,
            AccelerationDesc::TriangleRange(range) => self.triangle_input(*range)?,
            AccelerationDesc::Instances(instances) => {
                if instances.is_empty() {
                    return Err(GpuError::Contract("Empty TLAS".into()));
                }
                let (bytes, refs) = self.instance_data(instances)?;
                dependencies = refs;
                let buffer = self.create_buffer(
                    u64::from(capacity)
                        * std::mem::size_of::<vk::AccelerationStructureInstanceKHR>() as u64,
                    vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    true,
                )?;
                self.upload(buffer, &bytes)?;
                auxiliary.push(buffer);
                Geometry::Instances {
                    buffer,
                    count: u32::try_from(instances.len())
                        .map_err(|_| GpuError::Contract("Too many AS instances".into()))?,
                }
            }
        };
        let (native, count, kind) = self.as_geometry(&geometry)?;
        let geometries = [native];
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(kind)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .flags(build_flags(options))
            .geometries(&geometries);
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        // SAFETY: Sizes-only query uses checked geometry capacities; no GPU work is encoded.
        unsafe {
            self.context
                .acceleration
                .get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &info,
                    Some(&[if initial > 0 { capacity } else { count }]),
                    &mut sizes,
                );
        }
        let scratch = self.create_buffer(
            sizes.build_scratch_size.max(sizes.update_scratch_size)
                + self.context.scratch_alignment,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        auxiliary.push(scratch);
        let (handle, backing) = self
            .context
            .structure(sizes.acceleration_structure_size, kind)?;
        let address = if matches!(&geometry, Geometry::Triangles { .. }) {
            // SAFETY: This live BLAS and its backing allocation belong to the device.
            // Vulkan keeps the address valid until that same object is destroyed.
            unsafe {
                self.context
                    .acceleration
                    .get_acceleration_structure_device_address(
                        &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                            .acceleration_structure(handle),
                    )
            }
        } else {
            0
        };
        self.accelerations.insert(Owned {
            raw: NativeAcceleration {
                handle,
                address,
                backing,
                scratch,
                geometry,
                dependencies,
                auxiliary,
                built: false,
                built_count: 0,
                compacted: false,
                options,
                capacity,
            },
            last: [0; 2],
            heap: None,
            owner: Queue::Graphics,
        })
    }
    /// Publish a TLAS into the optional update-after-bind AS descriptor table.
    pub fn bind_acceleration_structure(
        &mut self,
        structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        let native = self.accelerations.get_mut(structure)?;
        if !matches!(native.raw.geometry, Geometry::Instances { .. }) {
            return Err(GpuError::Contract(
                "Only TLAS resources may be shader-bound".into(),
            ));
        }
        if let Some(index) = native.heap {
            return Ok(index);
        }
        let index = self.heap.acceleration(&self.context, native.raw.handle)?;
        native.heap = Some(index);
        Ok(index)
    }
    fn as_geometry(
        &self,
        geometry: &Geometry,
    ) -> Result<(
        vk::AccelerationStructureGeometryKHR<'static>,
        u32,
        vk::AccelerationStructureTypeKHR,
    )> {
        Ok(match *geometry {
            Geometry::Triangles {
                vertices,
                vertex_offset,
                vertex_count,
                stride,
                indices,
                index_offset,
                index_count,
            } => (
                vk::AccelerationStructureGeometryKHR::default()
                    .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
                    .flags(vk::GeometryFlagsKHR::OPAQUE)
                    .geometry(vk::AccelerationStructureGeometryDataKHR {
                        triangles: vk::AccelerationStructureGeometryTrianglesDataKHR::default()
                            .vertex_format(vk::Format::R32G32B32_SFLOAT)
                            .vertex_data(vk::DeviceOrHostAddressConstKHR {
                                device_address: self.range_address(vertices, vertex_offset)?,
                            })
                            .vertex_stride(stride)
                            .max_vertex(vertex_count - 1)
                            .index_type(vk::IndexType::UINT32)
                            .index_data(vk::DeviceOrHostAddressConstKHR {
                                device_address: self.range_address(indices, index_offset)?,
                            }),
                    }),
                index_count / 3,
                vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
            ),
            Geometry::Instances { buffer, count } => (
                vk::AccelerationStructureGeometryKHR::default()
                    .geometry_type(vk::GeometryTypeKHR::INSTANCES)
                    .geometry(vk::AccelerationStructureGeometryDataKHR {
                        instances: vk::AccelerationStructureGeometryInstancesDataKHR::default()
                            .data(vk::DeviceOrHostAddressConstKHR {
                                device_address: self.context.address(self.buffers.get(buffer)?.raw),
                            }),
                    }),
                count,
                vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            ),
        })
    }
    fn instance_data(
        &self,
        instances: &[AccelerationInstance],
    ) -> Result<(Vec<u8>, Vec<AccelerationStructure>)> {
        let mut dependencies = Vec::with_capacity(instances.len());
        let mut native = Vec::new();
        for instance in instances {
            let blas = self.accelerations.get(instance.geometry)?;
            if !matches!(blas.raw.geometry, Geometry::Triangles { .. })
                || instance.custom_index > 0xffffff
                || instance.transform.iter().any(|v| !v.is_finite())
            {
                return Err(GpuError::Contract("Invalid TLAS instance".into()));
            }
            native.push(vk::AccelerationStructureInstanceKHR {
                transform: vk::TransformMatrixKHR {
                    matrix: instance.transform,
                },
                instance_custom_index_and_mask: vk::Packed24_8::new(
                    instance.custom_index,
                    instance.mask,
                ),
                instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
                    0,
                    vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw() as u8,
                ),
                acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                    device_handle: blas.raw.address,
                },
            });
            dependencies.push(instance.geometry);
        }
        // SAFETY: Vulkan instance records are initialized repr(C) POD with no uninitialized padding.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                native.as_ptr().cast::<u8>(),
                std::mem::size_of_val(native.as_slice()),
            )
        };
        Ok((bytes.to_vec(), dependencies))
    }
    /// Queue new TLAS instance data on its owning queue. Rebuild afterward; count
    /// may change within the capacity reserved at creation.
    pub fn update_instances(
        &mut self,
        structure: AccelerationStructure,
        instances: &[AccelerationInstance],
    ) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        let Geometry::Instances { buffer, .. } = native.geometry else {
            return Err(GpuError::Contract("Instance updates require a TLAS".into()));
        };
        if !native.options.allow_update
            || instances.is_empty()
            || instances.len() > native.capacity as usize
        {
            return Err(GpuError::Contract(
                "Invalid dynamic TLAS instance count".into(),
            ));
        }
        let (bytes, dependencies) = self.instance_data(instances)?;
        self.upload_ordered(buffer, 0, &bytes)?;
        let native = &mut self.accelerations.get_mut(structure)?.raw;
        native.geometry = Geometry::Instances {
            buffer,
            count: instances.len() as u32,
        };
        native.dependencies = dependencies;
        Ok(())
    }
    pub(super) fn mark_as_built(
        &mut self,
        handle: AccelerationStructure,
        queue: Queue,
    ) -> Result<()> {
        let native = self.accelerations.get_mut(handle)?;
        native.owner = queue;
        native.raw.built = true;
        if let Geometry::Instances { buffer, count } = native.raw.geometry {
            native.raw.built_count = count;
            self.buffers.get_mut(buffer)?.owner = queue;
        }
        Ok(())
    }
    pub(super) fn validate_as_refit(&self, structure: AccelerationStructure) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        if !native.built
            || !native.options.allow_update
            || matches!(native.geometry,Geometry::Instances{count,..} if count != native.built_count)
        {
            return Err(GpuError::Contract(
                "Refit requires a built update-enabled AS".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn validate_as_build(&self, structure: AccelerationStructure) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        if native.built && (!native.options.allow_update || native.compacted) {
            return Err(GpuError::Contract(
                "Static AS already built; replacement requires a fresh identity".into(),
            ));
        }
        let _ = self.as_geometry(&native.geometry)?;
        Ok(())
    }
    pub(super) unsafe fn encode_as_build(
        &self,
        cmd: vk::CommandBuffer,
        structure: AccelerationStructure,
    ) -> Result<()> {
        // SAFETY: caller validated live handles and build capabilities before native recording.
        unsafe { self.encode_as_mode(cmd, structure, false) }
    }
    pub(super) unsafe fn encode_as_mode(
        &self,
        cmd: vk::CommandBuffer,
        structure: AccelerationStructure,
        refit: bool,
    ) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        let (geometry, count, kind) = self.as_geometry(&native.geometry)?;
        let address = self
            .context
            .address(self.buffers.get(native.scratch)?.raw)
            .next_multiple_of(self.context.scratch_alignment);
        let geometries = [geometry];
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(kind)
            .mode(if refit {
                vk::BuildAccelerationStructureModeKHR::UPDATE
            } else {
                vk::BuildAccelerationStructureModeKHR::BUILD
            })
            .src_acceleration_structure(if refit {
                native.handle
            } else {
                vk::AccelerationStructureKHR::null()
            })
            .flags(build_flags(native.options))
            .geometries(&geometries)
            .dst_acceleration_structure(native.handle)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: address,
            });
        let ranges = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
        // SAFETY: Creation queried exact sizes, padded/aligned scratch, and retained every input.
        unsafe {
            self.context.acceleration.cmd_build_acceleration_structures(
                cmd,
                &[info],
                &[Some(&ranges)],
            );
        }
        Ok(())
    }
}

fn build_flags(options: AccelerationOptions) -> vk::BuildAccelerationStructureFlagsKHR {
    let mut flags = vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE;
    if options.allow_update {
        flags |= vk::BuildAccelerationStructureFlagsKHR::ALLOW_UPDATE;
    }
    if options.allow_compaction {
        flags |= vk::BuildAccelerationStructureFlagsKHR::ALLOW_COMPACTION;
    }
    flags
}

unsafe fn as_dependency(device: &ash::Device, cmd: vk::CommandBuffer) {
    let memory = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
        .dst_access_mask(
            vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR
                | vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR,
        )];
    // SAFETY: caller supplies a recording command buffer outside rendering on the source's owning family.
    unsafe {
        device.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&memory));
    }
}
