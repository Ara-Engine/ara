//! Two pooled geometries and three real transformed instances share one upload; the
//! cooked `rangeProbe` ray query (SPIR-V on Vulkan, Metal Shader Converter on Metal)
//! reads each instance's ID back. Skips on devices without the RT tier.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Queue, Resource, RootConstants,
    acceleration::{AccelerationDesc, AccelerationInstance, TriangleRange},
    commands::Command,
    cooked::CookedPipeline,
    vk,
};

#[test]
fn pooled_triangle_ranges_keep_local_indices_and_instance_transforms() -> Result<()> {
    transfer_uploaded_ranges(Queue::Graphics)
}

#[test]
fn pooled_triangle_ranges_build_on_compute_after_transfer() -> Result<()> {
    transfer_uploaded_ranges(Queue::Compute)
}

fn transfer_uploaded_ranges(queue: Queue) -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join("../../target/ara-gpu/triangle-ranges"))?;
    if !gpu.info().ray_tracing {
        return Ok(());
    }
    let mut bytes = vec![0; 64];
    for x in [0.0, 2.0] {
        for position in [[x - 0.5f32, -0.5, 0.0], [x + 0.5, -0.5, 0.0], [x, 0.5, 0.0]] {
            bytes.extend_from_slice(bytemuck::cast_slice(&position));
            bytes.extend_from_slice(&[0; 52]);
        }
    }
    bytes.extend_from_slice(&[0; 16]);
    let first_indices = bytes.len() as u64;
    bytes.extend_from_slice(bytemuck::cast_slice(&[0u32, 1, 2, 2, 1, 0]));
    let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    let pool = gpu.create_buffer(
        bytes.len() as u64,
        usage | vk::BufferUsageFlags::TRANSFER_DST,
        false,
    )?;
    let staging =
        gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(staging, &bytes)?;
    let first = TriangleRange {
        vertices: pool,
        vertex_offset: 64,
        vertex_count: 3,
        vertex_stride: 64,
        indices: pool,
        index_offset: first_indices,
        index_count: 3,
    };
    for invalid in [
        TriangleRange {
            vertex_offset: 65,
            ..first
        },
        TriangleRange {
            index_offset: first_indices + 2,
            ..first
        },
        TriangleRange {
            vertex_offset: u64::MAX - 15,
            ..first
        },
        TriangleRange {
            vertex_count: 30,
            ..first
        },
        TriangleRange {
            index_count: 30,
            ..first
        },
    ] {
        assert!(
            gpu.create_acceleration_structure(&AccelerationDesc::TriangleRange(invalid))
                .is_err()
        );
    }
    let second = TriangleRange {
        vertex_offset: 256,
        index_offset: first_indices + 12,
        ..first
    };
    let blas = [
        gpu.create_acceleration_structure(&AccelerationDesc::TriangleRange(first))?,
        gpu.create_acceleration_structure(&AccelerationDesc::TriangleRange(second))?,
    ];
    let instances = [(blas[0], 11, 0.0), (blas[1], 23, 0.0), (blas[0], 47, 4.0)]
        .into_iter()
        .map(|(geometry, custom_index, x)| AccelerationInstance {
            geometry,
            custom_index,
            mask: 255,
            transform: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        })
        .collect();
    let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
    let output = gpu.create_buffer(12, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    let output_index = gpu.storage_buffer(output)?.index;
    let tlas_index = gpu.bind_acceleration_structure(tlas)?.index;
    let cooked = root.join("tests/cooked");
    let manifest: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("triangle_ranges.json"))?)?;
    let pipeline = gpu.create_pipeline(&manifest, &cooked)?;
    let mut commands = gpu.begin(queue);
    commands.push(Command::BeginPass("Scene.Range.BLAS.Build".into()));
    commands.barriers([
        edge(
            Resource::Buffer(staging),
            Access::HostWrite,
            Access::CopySource,
        ),
        edge(
            Resource::Buffer(pool),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ]);
    commands.push(Command::CopyBuffer {
        source: staging,
        destination: pool,
        size: bytes.len() as u64,
    });
    commands.barriers([edge(
        Resource::Buffer(pool),
        Access::CopyDestination,
        Access::AsInput,
    )]);
    for geometry in blas {
        commands.push(Command::BuildAccelerationStructure(geometry));
        commands.barriers([edge(
            Resource::AccelerationStructure(geometry),
            Access::AsBuild,
            Access::AsInput,
        )]);
    }
    commands.push(Command::EndPass);
    commands.push(Command::BuildAccelerationStructure(tlas));
    commands.barriers([
        edge(
            Resource::AccelerationStructure(tlas),
            Access::AsBuild,
            Access::AsRead,
        ),
        edge(Resource::Buffer(output), Access::Undefined, Access::Storage),
    ]);
    commands.push(Command::Pipeline(pipeline));
    commands.push(Command::Constants(RootConstants {
        words: [tlas_index, output_index, 0, 0, 0, 0, 0, 0],
    }));
    commands.push(Command::Dispatch([1, 1, 1]));
    commands.barriers([edge(
        Resource::Buffer(output),
        Access::Storage,
        Access::HostRead,
    )]);
    let point = gpu.submit(commands.finish(), &[])?;
    let timings = gpu.resolve(point)?;
    assert_eq!(
        gpu.read(output)?,
        bytemuck::cast_slice::<_, u8>(&[11u32, 23, 47])
    );
    eprintln!("pooled two-BLAS build: {:.6}ms", timings[0].milliseconds);
    for resource in [
        Resource::AccelerationStructure(tlas),
        Resource::AccelerationStructure(blas[0]),
        Resource::AccelerationStructure(blas[1]),
        Resource::Buffer(pool),
        Resource::Buffer(staging),
        Resource::Buffer(output),
    ] {
        gpu.retire(resource)?;
    }
    gpu.retire_pipeline(pipeline)?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}

fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
