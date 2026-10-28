//! Metal 4 acceleration structures behind `VulkanDevice`'s static BLAS/TLAS API: one
//! primitive structure per triangle range and an instance structure over BLAS resource
//! IDs, each built once on a compute encoder. Every structure is resident while alive,
//! since a TLAS reaches its BLASes only by ID. Shaders reach a TLAS the way Metal
//! Shader Converter expects inline ray queries to: the descriptor at root parameter 4
//! holds the address of a header with the TLAS resource ID and one hit-group
//! contribution per instance (`bind_acceleration_structure`).
use super::*;
use crate::acceleration::{AccelerationDesc, TriangleRange};
use objc2_foundation::NSArray;

/// MSC's `IRRaytracingAccelerationStructureGPUHeader` (runtime header): the TLAS
/// resource ID, then the address of one u32 hit-group contribution per instance.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Header {
    acceleration_structure_id: u64,
    instance_contributions: u64,
    /// Padding to the runtime header's 64 bytes.
    pad: [u64; 6],
}

pub(super) enum Geometry {
    Triangles(TriangleRange),
    Instances { descriptors: Buffer, count: u32 },
}

/// One structure and the storage its build and shader binding own.
pub(super) struct NativeAcceleration {
    pub structure: Retained<ProtocolObject<dyn MTLAccelerationStructure>>,
    geometry: Geometry,
    scratch: Buffer,
    /// BLASes a TLAS references; they cannot retire while it lives.
    pub dependencies: Vec<AccelerationStructure>,
    /// Buffers retired with the structure: scratch, instance descriptors, the header.
    pub auxiliary: Vec<Buffer>,
    pub built: bool,
}

impl MetalDevice {
    /// Like `VulkanDevice::create_acceleration_structure`: allocate a static BLAS or
    /// TLAS and its scratch; build it once through `Command::BuildAccelerationStructure`.
    pub(super) fn create_acceleration(
        &mut self,
        desc: &AccelerationDesc,
    ) -> Result<AccelerationStructure> {
        if !self.info.ray_tracing {
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
                let mut native = Vec::with_capacity(instances.len());
                for instance in instances {
                    let blas = &self.accelerations.get(instance.geometry)?.raw;
                    if !matches!(blas.geometry, Geometry::Triangles(_))
                        || instance.custom_index > 0xffffff
                        || instance.transform.iter().any(|v| !v.is_finite())
                    {
                        return Err(GpuError::Contract("Invalid TLAS instance".into()));
                    }
                    let t = instance.transform;
                    let column = |c: usize| MTLPackedFloat3 {
                        x: t[c],
                        y: t[4 + c],
                        z: t[8 + c],
                    };
                    native.push(MTLIndirectAccelerationStructureInstanceDescriptor {
                        transformationMatrix: MTLPackedFloat4x3 {
                            columns: [column(0), column(1), column(2), column(3)],
                        },
                        // Opaque, double-sided geometry, as on Vulkan.
                        options: MTLAccelerationStructureInstanceOptions::Opaque
                            | MTLAccelerationStructureInstanceOptions::DisableTriangleCulling,
                        mask: u32::from(instance.mask),
                        intersectionFunctionTableOffset: 0,
                        userID: instance.custom_index,
                        accelerationStructureID: blas.structure.gpuResourceID(),
                    });
                    dependencies.push(instance.geometry);
                }
                // SAFETY: the descriptors are initialized repr(C) records without padding.
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        native.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(native.as_slice()),
                    )
                };
                let descriptors = self.create_buffer(
                    bytes.len() as u64,
                    vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    true,
                )?;
                self.upload(descriptors, bytes)?;
                auxiliary.push(descriptors);
                Geometry::Instances {
                    descriptors,
                    count: u32::try_from(instances.len())
                        .map_err(|_| GpuError::Contract("Too many AS instances".into()))?,
                }
            }
        };
        let descriptor = self.descriptor(&geometry)?;
        let sizes = self
            .device
            .accelerationStructureSizesWithDescriptor(&descriptor);
        let scratch = self.create_buffer(
            (sizes.buildScratchBufferSize as u64).max(4),
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        )?;
        auxiliary.push(scratch);
        let structure = self
            .device
            .newAccelerationStructureWithSize(sizes.accelerationStructureSize)
            .ok_or_else(|| nil("acceleration structure"))?;
        self.residency
            .addAllocation(ProtocolObject::from_ref(&*structure));
        self.residency_dirty = true;
        self.accelerations.insert(Owned::new(NativeAcceleration {
            structure,
            geometry,
            scratch,
            dependencies,
            auxiliary,
            built: false,
        }))
    }

    /// Like `VulkanDevice::bind_acceleration_structure`: publish a TLAS in the
    /// acceleration-structure table (root parameter 4) through its converter header.
    pub fn bind_acceleration_structure(
        &mut self,
        structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        let native = &self.accelerations.get(structure)?;
        let Geometry::Instances { count, .. } = native.raw.geometry else {
            return Err(GpuError::Contract(
                "Only TLAS resources may be shader-bound".into(),
            ));
        };
        if let Some(index) = native.heap {
            return Ok(index);
        }
        let id = native.raw.structure.gpuResourceID().to_raw();
        let size = std::mem::size_of::<Header>() as u64;
        let header = self.create_buffer(
            size + u64::from(count) * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let address = self.buffers.get(header)?.raw.buffer.gpuAddress();
        // Ray queries use no hit groups: every instance contributes zero.
        let mut bytes = bytemuck::bytes_of(&Header {
            acceleration_structure_id: id,
            instance_contributions: address + size,
            pad: [0; 6],
        })
        .to_vec();
        bytes.resize(bytes.len() + count as usize * 4, 0);
        self.upload(header, &bytes)?;
        let entry = abi::TableEntry {
            gpu_va: address,
            ..Default::default()
        };
        let index = self.tables.accelerations.insert(entry)?;
        let native = self.accelerations.get_mut(structure)?;
        native.raw.auxiliary.push(header);
        native.heap = Some(index);
        Ok(index)
    }

    /// Encode a structure's one build into the open compute encoder.
    pub(super) fn encode_as_build(
        &self,
        encoder: &ProtocolObject<dyn MTL4ComputeCommandEncoder>,
        structure: AccelerationStructure,
    ) -> Result<()> {
        let native = &self.accelerations.get(structure)?.raw;
        if native.built {
            return Err(GpuError::Contract(
                "Static AS already built; replacement requires a fresh identity".into(),
            ));
        }
        let descriptor = self.descriptor(&native.geometry)?;
        let scratch = &self.buffers.get(native.scratch)?.raw;
        // SAFETY: creation sized the structure and scratch for this descriptor; both,
        // the inputs and every BLAS a TLAS references are resident and retained.
        unsafe {
            encoder.buildAccelerationStructure_descriptor_scratchBuffer(
                &native.structure,
                &descriptor,
                MTL4BufferRange {
                    bufferAddress: scratch.buffer.gpuAddress(),
                    length: scratch.size,
                },
            );
        }
        Ok(())
    }

    /// Record builds as done once their submission is committed (static structures).
    pub(super) fn mark_built(&mut self, commands: &[crate::commands::Command]) -> Result<()> {
        for command in commands {
            if let crate::commands::Command::BuildAccelerationStructure(h) = command {
                self.accelerations.get_mut(*h)?.raw.built = true;
            }
        }
        Ok(())
    }

    fn descriptor(
        &self,
        geometry: &Geometry,
    ) -> Result<Retained<MTL4AccelerationStructureDescriptor>> {
        Ok(match *geometry {
            Geometry::Triangles(range) => {
                let range_of = |buffer: Buffer, offset: u64, length: u64| -> Result<_> {
                    Ok(MTL4BufferRange {
                        bufferAddress: self.buffers.get(buffer)?.raw.buffer.gpuAddress() + offset,
                        length,
                    })
                };
                let triangles = MTL4AccelerationStructureTriangleGeometryDescriptor::new();
                let vertices = range_of(
                    range.vertices,
                    range.vertex_offset,
                    u64::from(range.vertex_count) * range.vertex_stride,
                )?;
                let indices = range_of(
                    range.indices,
                    range.index_offset,
                    u64::from(range.index_count) * 4,
                )?;
                // SAFETY: `triangle_input` checked both ranges against their buffers.
                unsafe {
                    triangles.setVertexBuffer(vertices);
                    triangles.setVertexStride(range.vertex_stride as usize);
                    triangles.setIndexBuffer(indices);
                    triangles.setTriangleCount(range.index_count as usize / 3);
                }
                triangles.setVertexFormat(MTLAttributeFormat::Float3);
                triangles.setIndexType(MTLIndexType::UInt32);
                triangles.setOpaque(true);
                let descriptor = MTL4PrimitiveAccelerationStructureDescriptor::new();
                descriptor.setGeometryDescriptors(Some(&NSArray::from_retained_slice(&[
                    Retained::into_super(triangles),
                ])));
                Retained::into_super(descriptor)
            }
            Geometry::Instances { descriptors, count } => {
                let buffer = &self.buffers.get(descriptors)?.raw;
                let descriptor = MTL4InstanceAccelerationStructureDescriptor::new();
                // SAFETY: the buffer holds `count` indirect instance descriptors.
                unsafe {
                    descriptor.setInstanceDescriptorBuffer(MTL4BufferRange {
                        bufferAddress: buffer.buffer.gpuAddress(),
                        length: buffer.size,
                    });
                    descriptor.setInstanceDescriptorStride(std::mem::size_of::<
                        MTLIndirectAccelerationStructureInstanceDescriptor,
                    >());
                    descriptor.setInstanceCount(count as usize);
                }
                descriptor.setInstanceDescriptorType(
                    MTLAccelerationStructureInstanceDescriptorType::Indirect,
                );
                Retained::into_super(descriptor)
            }
        })
    }

    fn triangle_input(&self, range: TriangleRange) -> Result<Geometry> {
        let (vertices, indices) = (
            &self.buffers.get(range.vertices)?.raw,
            &self.buffers.get(range.indices)?.raw,
        );
        range.validate(
            (vertices.size, vertices.usage),
            (indices.size, indices.usage),
        )?;
        Ok(Geometry::Triangles(range))
    }
}
