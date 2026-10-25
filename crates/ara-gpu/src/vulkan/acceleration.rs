use super::*;
use crate::acceleration::{AccelerationDesc, TriangleRange};
mod triangles;

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
    geometry: Geometry,
}
impl VulkanDevice {
    /// Allocate a static BLAS/TLAS and aligned scratch. Build it through a typed command.
    /// Acceleration structures are optional; the raster tier never requires RT support.
    pub fn create_acceleration_structure(
        &mut self,
        desc: &AccelerationDesc,
    ) -> Result<AccelerationStructure> {
        crate::cpu_scope!("GPU.allocate_acceleration_structure");
        if !self.context.info.ray_tracing {
            return Err(GpuError::Contract(
                "RT-tier acceleration structures are unavailable".into(),
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
                            vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw()
                                as u8,
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
                let buffer = self.create_buffer(
                    bytes.len() as u64,
                    vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    true,
                )?;
                self.upload(buffer, bytes)?;
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
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .geometries(&geometries);
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        // SAFETY: Sizes-only query uses checked geometry capacities; no GPU work is encoded.
        unsafe {
            self.context
                .acceleration
                .get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &info,
                    Some(&[count]),
                    &mut sizes,
                );
        }
        let scratch = self.create_buffer(
            sizes.build_scratch_size + self.context.scratch_alignment,
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
    pub(super) fn validate_as_build(&self, structure: AccelerationStructure) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        if native.built {
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
        let native = &self.accelerations.get(structure)?.raw;
        let (geometry, count, kind) = self.as_geometry(&native.geometry)?;
        let address = self
            .context
            .address(self.buffers.get(native.scratch)?.raw)
            .next_multiple_of(self.context.scratch_alignment);
        let geometries = [geometry];
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(kind)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
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
