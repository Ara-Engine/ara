//! A native query's ordinal/primitive selects exact positions in retained padded mesh ranges.
use super::*;
use crate::{
    acceleration::{AccelerationDesc, AccelerationInstance, TriangleRange},
    commands::Command,
    hit_geometry::HitGeometry,
};
#[test]
fn hit_positions_fetch_known_indexed_geometry_without_the_intrinsic() {
    let mut gpu = MetalDevice::new(&[], std::env::temp_dir()).unwrap();
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
        .create_acceleration(&AccelerationDesc::TriangleRange(range))
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
        .create_acceleration(&AccelerationDesc::Instances(instances))
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
    let source = format!(
        r#"
#include <metal_raytracing>
using namespace metal::raytracing;
#define ARA_HIT_CONTEXT constant TopLevel& top,
#define ARA_HIT_LOAD(slot,offset) top.buffers[slot].data[(offset)/4]
#define ARA_HIT_FLOAT3(a,b,c) as_type<float3>(uint3(a,b,c))
{}
struct Header {{instance_acceleration_structure scene; device uint* contributions;}};
struct Entry {{device Header* header; ulong resource_id; ulong metadata;}};
kernel void hitPositions(constant TopLevel& top [[buffer(2)]]) {{
    device Entry* entries=(device Entry*)top.accelerations;
    intersector<triangle_data,instancing> query;
    query.force_opacity(forced_opacity::opaque);
    auto hit=query.intersect(ray(float3(1.5,0,-2),float3(0,0,1),0.001,100),entries[top.root[0]].header->scene);
    device uint* out=top.buffers[top.root[2]].data;
    if (hit.type != intersection_type::triangle) {{out[0]=0xffffffff;return;}}
    auto tri=araHitTriangle(top,top.root[1],0,2,hit.instance_id,hit.primitive_id);
    out[0]=hit.instance_id;out[1]=hit.primitive_id;out[2]=tri.valid;
    float3 p[3]={{tri.a,tri.b,tri.c}};
    for(uint i=0;i<3;i++) for(uint j=0;j<3;j++) out[3+i*3+j]=as_type<uint>(p[i][j]);
    out[12]=araHitTriangle(top,top.root[1],0,2,2,0).valid;
    out[13]=araHitTriangle(top,top.root[1],0,2,1,2).valid;
}}
"#,
        crate::hit_geometry::SHADER
    );
    let pipeline = gpu.msl_pipeline_from(
        &source,
        &[(cooked::Stage::Compute, "hitPositions")],
        &cooked::RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
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
}
