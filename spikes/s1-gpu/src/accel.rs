use crate::{
    gpu::{Buffer, Gpu},
    scene::{Scene, Vertex},
};
use anyhow::Result;
use ash::vk;

/// Built scene TLAS and startup timestamp evidence.
pub struct Acceleration {
    pub tlas: vk::AccelerationStructureKHR,
    pub build_ms: f64,
    pub triangles: u32,
}

/// Builds a BLAS containing all static scene triangles, then a one-instance TLAS.
pub fn build(gpu: &mut Gpu, scene: &Scene) -> Result<Acceleration> {
    let geometry = vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
        .flags(vk::GeometryFlagsKHR::OPAQUE)
        .geometry(vk::AccelerationStructureGeometryDataKHR {
            triangles: vk::AccelerationStructureGeometryTrianglesDataKHR::default()
                .vertex_format(vk::Format::R32G32B32_SFLOAT)
                .vertex_data(vk::DeviceOrHostAddressConstKHR {
                    device_address: gpu.address(scene.vertices),
                })
                .vertex_stride(std::mem::size_of::<Vertex>() as u64)
                .max_vertex(scene.vertex_count - 1)
                .index_type(vk::IndexType::UINT32)
                .index_data(vk::DeviceOrHostAddressConstKHR {
                    device_address: gpu.address(scene.indices),
                }),
        });
    let triangles = scene.draws.iter().map(|d| d.count / 3).sum();
    let (blas, blas_scratch) = prepare(
        gpu,
        geometry,
        triangles,
        vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
    )?;
    // SAFETY: BLAS handle was created by this device and remains retained in Gpu.
    let blas_address = unsafe {
        gpu.acceleration.get_acceleration_structure_device_address(
            &vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(blas),
        )
    };
    let instance = vk::AccelerationStructureInstanceKHR {
        transform: vk::TransformMatrixKHR {
            matrix: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
        },
        instance_custom_index_and_mask: vk::Packed24_8::new(0, 0xff),
        instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
            0,
            vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw() as u8,
        ),
        acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
            device_handle: blas_address,
        },
    };
    let instance_buffer = gpu.buffer(
        std::mem::size_of_val(&instance) as u64,
        vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        true,
    )?;
    // SAFETY: The Vulkan C struct is fully initialized and copied immediately as bytes.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&instance as *const vk::AccelerationStructureInstanceKHR).cast(),
            std::mem::size_of_val(&instance),
        )
    };
    gpu.upload(instance_buffer, bytes)?;
    let instances = vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR {
            instances: vk::AccelerationStructureGeometryInstancesDataKHR::default().data(
                vk::DeviceOrHostAddressConstKHR {
                    device_address: gpu.address(instance_buffer),
                },
            ),
        });
    let (tlas, tlas_scratch) = prepare(
        gpu,
        instances,
        1,
        vk::AccelerationStructureTypeKHR::TOP_LEVEL,
    )?;
    // SAFETY: Query pool is local and destroyed after the synchronous submission.
    let queries = unsafe {
        gpu.device.create_query_pool(
            &vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(2),
            None,
        )?
    };
    gpu.immediate(|cmd| {
        // SAFETY: Geometry, instances, scratch and AS buffers remain alive. Barriers
        // order BLAS write -> TLAS read -> fragment ray-query read.
        unsafe {
            gpu.device.cmd_reset_query_pool(cmd, queries, 0, 2);
            gpu.device
                .cmd_write_timestamp2(cmd, vk::PipelineStageFlags2::TOP_OF_PIPE, queries, 0);
            record(
                gpu,
                cmd,
                geometry,
                blas,
                blas_scratch,
                triangles,
                vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
            );
            let barrier = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
                .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
                .dst_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
                .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR);
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&[barrier]),
            );
            record(
                gpu,
                cmd,
                instances,
                tlas,
                tlas_scratch,
                1,
                vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            );
            let barrier = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
                .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR);
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&[barrier]),
            );
            gpu.device.cmd_write_timestamp2(
                cmd,
                vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                queries,
                1,
            );
        }
    })?;
    let mut timestamps = [0_u64; 2];
    // SAFETY: Submission completed, both queries have been written.
    unsafe {
        gpu.device.get_query_pool_results(
            queries,
            0,
            &mut timestamps,
            vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
        )?;
        gpu.device.destroy_query_pool(queries, None);
    }
    Ok(Acceleration {
        tlas,
        triangles,
        build_ms: (timestamps[1] - timestamps[0]) as f64 * f64::from(gpu.info.timestamp_period_ns)
            / 1e6,
    })
}

fn prepare(
    gpu: &mut Gpu,
    geometry: vk::AccelerationStructureGeometryKHR<'_>,
    count: u32,
    kind: vk::AccelerationStructureTypeKHR,
) -> Result<(vk::AccelerationStructureKHR, Buffer)> {
    let geometries = [geometry];
    let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(kind)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(&geometries);
    let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    // SAFETY: Sizes-only query uses a valid geometry description and primitive count.
    unsafe {
        gpu.acceleration.get_acceleration_structure_build_sizes(
            vk::AccelerationStructureBuildTypeKHR::DEVICE,
            &info,
            Some(&[count]),
            &mut sizes,
        );
    }
    let structure = gpu.structure(sizes.acceleration_structure_size, kind)?;
    let scratch = gpu.buffer(
        sizes.build_scratch_size + gpu.scratch_alignment,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        false,
    )?;
    Ok((structure, scratch))
}

fn record(
    gpu: &Gpu,
    cmd: vk::CommandBuffer,
    geometry: vk::AccelerationStructureGeometryKHR<'_>,
    target: vk::AccelerationStructureKHR,
    scratch: Buffer,
    count: u32,
    kind: vk::AccelerationStructureTypeKHR,
) {
    let address = gpu.address(scratch).next_multiple_of(gpu.scratch_alignment);
    let geometries = [geometry];
    let build = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(kind)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .dst_acceleration_structure(target)
        .geometries(&geometries)
        .scratch_data(vk::DeviceOrHostAddressKHR {
            device_address: address,
        });
    let ranges = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
    // SAFETY: prepare allocated queried sizes plus alignment padding; all referenced
    // input resources are live and previous build accesses are ordered by caller.
    unsafe {
        gpu.acceleration
            .cmd_build_acceleration_structures(cmd, &[build], &[Some(&ranges)]);
    }
}
