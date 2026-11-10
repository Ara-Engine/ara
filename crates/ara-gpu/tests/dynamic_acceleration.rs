//! The same cooked ray-query probe exercises dynamic AS on Vulkan and Metal.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Queue, Resource, RootConstants,
    acceleration::{AccelerationDesc, AccelerationInstance, AccelerationOptions},
    commands::Command,
    cooked::CookedPipeline,
    vk,
};
#[test]
fn dynamic_acceleration_on_graphics() -> Result<()> {
    dynamic(Queue::Graphics)
}
#[test]
fn dynamic_acceleration_on_compute() -> Result<()> {
    dynamic(Queue::Compute)
}
fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
fn dynamic(queue: Queue) -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join("../../target/ara-gpu/dynamic-acceleration"))?;
    if !gpu.info().ray_tracing {
        return Ok(());
    }
    let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    let positions = [[-1f32, -1., 0.], [1., -1., 0.], [0., 1., 0.]];
    let vertices = gpu.create_buffer(36, usage, true)?;
    gpu.upload(vertices, bytemuck::cast_slice(&positions))?;
    let indices = gpu.create_buffer(12, usage, true)?;
    gpu.upload(indices, bytemuck::cast_slice(&[0u32, 1, 2]))?;
    let options = AccelerationOptions {
        allow_update: true,
        allow_compaction: true,
        instance_capacity: 0,
    };
    let blas = gpu.create_acceleration_with_options(
        &AccelerationDesc::Triangles {
            vertices,
            vertex_count: 3,
            vertex_stride: 12,
            indices,
            index_count: 3,
        },
        options,
    )?;
    let mut rec = gpu.begin(queue);
    rec.barriers([
        edge(
            Resource::Buffer(vertices),
            Access::HostWrite,
            Access::AsInput,
        ),
        edge(
            Resource::Buffer(indices),
            Access::HostWrite,
            Access::AsInput,
        ),
    ]);
    rec.push(Command::BuildAccelerationStructure(blas));
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    let original = gpu.acceleration_size(blas)?;
    let compact = gpu.compact_acceleration_structure(blas)?;
    assert!(gpu.acceleration_size(compact)? <= original);
    gpu.retire(Resource::AccelerationStructure(blas))?;
    let blas = compact;
    let identity = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];
    let make = |i, transform| AccelerationInstance {
        geometry: blas,
        transform,
        custom_index: 11,
        mask: if i == 0 { 255 } else { 0 },
    };
    let tlas = gpu.create_acceleration_with_options(
        &AccelerationDesc::Instances(vec![make(0, identity)]),
        AccelerationOptions {
            instance_capacity: 4096,
            ..options
        },
    )?;
    let tlas_slot = gpu.bind_acceleration_structure(tlas)?.index;
    let output = gpu.create_buffer(12, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    let out = gpu.storage_buffer(output)?.index;
    let cooked = root.join("tests/cooked");
    let manifest: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("triangle_ranges.json"))?)?;
    let pipeline = gpu.create_pipeline(&manifest, &cooked)?;
    let trace = |gpu: &mut GpuDevice, refit: bool| -> Result<Vec<u8>> {
        let mut rec = gpu.begin(queue);
        rec.barriers([edge(
            Resource::AccelerationStructure(tlas),
            Access::AsRead,
            Access::AsBuild,
        )]);
        if refit {
            rec.barriers([
                edge(
                    Resource::Buffer(vertices),
                    Access::HostWrite,
                    Access::AsInput,
                ),
                edge(
                    Resource::AccelerationStructure(blas),
                    Access::AsInput,
                    Access::AsBuild,
                ),
            ]);
            rec.push(Command::RefitAccelerationStructure(blas));
        }
        rec.barriers([edge(
            Resource::AccelerationStructure(blas),
            Access::AsBuild,
            Access::AsInput,
        )]);
        rec.push(Command::BuildAccelerationStructure(tlas));
        rec.barriers([
            edge(
                Resource::AccelerationStructure(tlas),
                Access::AsBuild,
                Access::AsRead,
            ),
            edge(Resource::Buffer(output), Access::HostRead, Access::Storage),
        ]);
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [tlas_slot, out, 0, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::HostRead,
        )]);
        let p = gpu.submit(rec.finish(), &[])?;
        gpu.wait(p)?;
        Ok(gpu.read(output)?)
    };
    assert_eq!(
        trace(&mut gpu, false)?,
        bytemuck::cast_slice::<_, u8>(&[11u32, u32::MAX, u32::MAX])
    );
    let mut moved = identity;
    moved[3] = 2.;
    let instances: Vec<_> = (0..4096).map(|i| make(i, moved)).collect();
    gpu.update_instances(tlas, &instances)?;
    assert_eq!(
        trace(&mut gpu, false)?,
        bytemuck::cast_slice::<_, u8>(&[u32::MAX, 11u32, u32::MAX])
    );
    gpu.upload(
        vertices,
        bytemuck::cast_slice(&positions.map(|mut p| {
            p[0] += 20.;
            p
        })),
    )?;
    assert_eq!(
        trace(&mut gpu, true)?,
        bytemuck::cast_slice::<_, u8>(&[u32::MAX; 3])
    );
    gpu.assert_clean()?;
    Ok(())
}
