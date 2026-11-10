#![allow(clippy::unwrap_used)]
//! A real query fetches original indexed positions through the shared shader helper.
use ara_gpu::{
    acceleration::{AccelerationDesc, AccelerationInstance, TriangleRange},
    commands::Command,
    hit_geometry::HitGeometry,
    *,
};
fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
#[test]
fn hit_positions_fetch_known_indexed_geometry_without_the_intrinsic() {
    let mut gpu = GpuDevice::new(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/hit-geometry"),
    )
    .unwrap();
    let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
        | vk::BufferUsageFlags::STORAGE_BUFFER;
    let points: [[f32; 3]; 6] = [
        [19., -1., 2.],
        [21., -1., 2.],
        [20., 1., 2.],
        [-0.75, -0.5, 2.],
        [0.75, -0.5, 2.],
        [0., 0.75, 2.],
    ];
    let mut bytes = vec![0; 16 + 6 * 20];
    for (i, p) in points.iter().enumerate() {
        bytes[16 + i * 20..28 + i * 20].copy_from_slice(bytemuck::cast_slice(p));
    }
    let vertices = gpu.create_buffer(bytes.len() as u64, usage, true).unwrap();
    gpu.upload(vertices, &bytes).unwrap();
    let indices = gpu.create_buffer(36, usage, true).unwrap();
    gpu.upload(
        indices,
        bytemuck::cast_slice(&[99u32, 99, 99, 0, 1, 2, 5, 3, 4]),
    )
    .unwrap();
    let range = TriangleRange {
        vertices,
        vertex_offset: 16,
        vertex_count: 6,
        vertex_stride: 20,
        indices,
        index_offset: 12,
        index_count: 6,
    };
    let blas = gpu
        .create_acceleration_structure(&AccelerationDesc::TriangleRange(range))
        .unwrap();
    let mut instances = Vec::new();
    for i in 0..2 {
        instances.push(AccelerationInstance {
            geometry: blas,
            transform: if i == 1 {
                [-1., 0., 0., 1.5, 0., 2., 0., 0., 0., 0., 1., -2.]
            } else {
                [1., 0., 0., 20., 0., 1., 0., 0., 0., 0., 1., 0.]
            },
            custom_index: 91 + i,
            mask: if i == 0 { 0 } else { 255 },
        });
    }
    let tlas = gpu
        .create_acceleration_structure(&AccelerationDesc::Instances(instances))
        .unwrap();
    let geometry = HitGeometry::publish(&mut gpu, range).unwrap();
    let table = gpu
        .create_buffer(64, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let zero = bytemuck::Zeroable::zeroed();
    gpu.upload(table, bytemuck::cast_slice(&[zero, geometry]))
        .unwrap();
    let table_index = gpu.storage_buffer(table).unwrap();
    let output = gpu
        .create_buffer(64, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let out_index = gpu.storage_buffer(output).unwrap();
    let scene = gpu.bind_acceleration_structure(tlas).unwrap();
    let cooked = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/feature_cooked");
    let metadata: cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("hit.json")).unwrap()).unwrap();
    let pipeline = gpu.create_pipeline(&metadata, &cooked).unwrap();
    gpu.upload(output, &[0; 64]).unwrap();
    let mut rec = gpu.begin(Queue::Graphics);
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
        edge(
            Resource::Buffer(table),
            Access::HostWrite,
            Access::ShaderRead,
        ),
        edge(Resource::Buffer(output), Access::HostWrite, Access::Storage),
    ]);
    rec.push(Command::BuildAccelerationStructure(blas));
    rec.barriers([Barrier {
        resource: Resource::AccelerationStructure(blas),
        before: Access::AsBuild,
        after: Access::AsInput,
        handoff: None,
    }]);
    rec.push(Command::BuildAccelerationStructure(tlas));
    rec.barriers([Barrier {
        resource: Resource::AccelerationStructure(tlas),
        before: Access::AsBuild,
        after: Access::AsRead,
        handoff: None,
    }]);
    rec.push(Command::Pipeline(pipeline));
    rec.push(Command::Constants(RootConstants {
        words: [
            scene.index,
            table_index.index,
            out_index.index,
            0,
            0,
            0,
            0,
            0,
        ],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    rec.barriers([edge(
        Resource::Buffer(output),
        Access::Storage,
        Access::HostRead,
    )]);
    let p = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(p).unwrap();
    let actual = gpu.read(output).unwrap();
    let words: Vec<u32> = actual
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    assert_eq!(&words[..3], &[1, 1, 1]);
    let expected = [points[5], points[3], points[4]];
    assert_eq!(&actual[12..48], bytemuck::cast_slice::<_, u8>(&expected));
    assert_eq!(&words[12..14], &[0, 0]);
    gpu.assert_clean().unwrap();
}
