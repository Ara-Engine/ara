//! Ray-query timing on the Metal RT tier, opt-in:
//! `cargo test -p ara-gpu --lib -- --ignored ray_query_timing --nocapture`.
//! A 524k-triangle BLAS instanced 64 times is traced by 1920x1080 primary rays (2 s of
//! serialized dispatches, for steady clocks and an attached Metal System Trace) from a
//! runtime-MSL kernel that reads the TLAS through the same converter header cooked
//! shaders use (`bind_acceleration_structure`).
use super::*;
use crate::acceleration::{AccelerationDesc, AccelerationInstance};
use crate::commands::Command;
use crate::cooked::{RenderState, Stage};

const TRACE: &str = r#"
#include <metal_raytracing>
using namespace metal::raytracing;
struct Header { instance_acceleration_structure scene; device uint* contributions; };
struct AccelerationEntry { device Header* header; ulong resource_id; ulong metadata; };
kernel void trace(constant TopLevel& top [[buffer(2)]], uint2 id [[thread_position_in_grid]]) {
    uint width = top.root[2], height = top.root[3];
    if (id.x >= width || id.y >= height) return;
    float2 at = (float2(id) + 0.5f) / float2(width, height) * 16.f - 8.f;
    ray primary(float3(at, -10.f), float3(0.f, 0.f, 1.f), 0.001f, 100.f);
    intersector<triangle_data, instancing> query;
    query.force_opacity(forced_opacity::opaque);
    device AccelerationEntry* entries = (device AccelerationEntry*)top.accelerations;
    auto hit = query.intersect(primary, entries[top.root[0]].header->scene);
    top.buffers[top.root[1]].data[id.y * width + id.x] =
        hit.type == intersection_type::triangle ? hit.instance_id + 1 : 0;
}
"#;

/// A `quads` x `quads` grid over [-1, 1]^2 at z = 0: positions and U32 indices.
fn grid(quads: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let side = quads + 1;
    let positions = (0..side * side)
        .map(|i| {
            let (x, y) = (i % side, i / side);
            let at = |v: u32| v as f32 / quads as f32 * 2. - 1.;
            [at(x), at(y), 0.]
        })
        .collect();
    let indices = (0..quads * quads)
        .flat_map(|q| {
            let corner = q / quads * side + q % quads;
            [
                corner,
                corner + 1,
                corner + side,
                corner + 1,
                corner + side + 1,
                corner + side,
            ]
        })
        .collect();
    (positions, indices)
}

#[test]
#[ignore = "timing; run explicitly on an RT-tier Mac"]
fn ray_query_timing() {
    let mut gpu = MetalDevice::new(&[], std::env::temp_dir()).expect("Metal 4 device");
    if !gpu.info().ray_tracing {
        eprintln!("no RT tier on {}", gpu.info().name);
        return;
    }
    let (positions, indices) = grid(512);
    let input = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    let mut upload = |bytes: &[u8]| {
        let buffer = gpu
            .create_buffer(bytes.len() as u64, input, true)
            .expect("input buffer");
        gpu.upload(buffer, bytes).expect("upload");
        buffer
    };
    let vertices = upload(bytemuck::cast_slice(&positions));
    let index_buffer = upload(bytemuck::cast_slice(&indices));
    let blas = gpu
        .create_acceleration(&AccelerationDesc::Triangles {
            vertices,
            vertex_count: positions.len() as u32,
            vertex_stride: 12,
            indices: index_buffer,
            index_count: indices.len() as u32,
        })
        .expect("BLAS");
    let instances = (0..64)
        .map(|i| {
            let [x, y] = [i % 8, i / 8].map(|v| v as f32 * 2. - 7.);
            AccelerationInstance {
                geometry: blas,
                transform: [1., 0., 0., x, 0., 1., 0., y, 0., 0., 1., 0.],
                custom_index: i,
                mask: 255,
            }
        })
        .collect();
    let tlas = gpu
        .create_acceleration(&AccelerationDesc::Instances(instances))
        .expect("TLAS");
    let scene = gpu.bind_acceleration_structure(tlas).expect("bind").index;
    let [width, height] = [1920_u32, 1080];
    let output = gpu
        .create_buffer(
            u64::from(width * height) * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )
        .expect("output");
    let slot = gpu.storage_buffer(output).expect("slot").index;
    let pipeline = gpu.msl_pipeline_from(
        TRACE,
        &[(Stage::Compute, "trace")],
        &RenderState::default(),
        Some([8, 8, 1]),
    );
    let edge = |resource, before, after| {
        Command::Barrier(Barrier {
            resource,
            before,
            after,
            handoff: None,
        })
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginPass("RT.BLAS".into()));
    rec.push(Command::BuildAccelerationStructure(blas));
    rec.push(Command::EndPass);
    let blas_ready = edge(
        Resource::AccelerationStructure(blas),
        Access::AsBuild,
        Access::AsInput,
    );
    rec.push(blas_ready);
    rec.push(Command::BeginPass("RT.TLAS".into()));
    rec.push(Command::BuildAccelerationStructure(tlas));
    rec.push(Command::EndPass);
    rec.push(edge(
        Resource::AccelerationStructure(tlas),
        Access::AsBuild,
        Access::AsRead,
    ));
    let trace = |rec: &mut Recorder| {
        rec.push(Command::BeginPass("RT.Trace".into()));
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [scene, slot, width, height, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([width / 8, height.div_ceil(8), 1]));
        rec.push(Command::EndPass);
    };
    let serialize = |rec: &mut Recorder| {
        rec.push(edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::Storage,
        ));
    };
    trace(&mut rec);
    serialize(&mut rec);
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    let timings = gpu.resolve(point).expect("resolve");
    let ids = words(&gpu.read(output).expect("read"));
    let hits = ids.iter().filter(|&&id| id > 0).count();
    assert!(
        hits as f64 > 0.99 * ids.len() as f64,
        "{hits} of {} rays hit the tiled grid",
        ids.len()
    );
    assert!(ids.iter().all(|&id| id <= 64), "instance IDs stay in range");
    let ms = |timings: &[PassTiming], name: &str| -> Vec<f64> {
        timings
            .iter()
            .filter(|t| t.pass == name)
            .map(|t| t.milliseconds)
            .collect()
    };
    // Two seconds of serialized traces, ten per submission: steady clocks, and long
    // enough for an attached Metal System Trace to see them.
    let mut traced = Vec::new();
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(2) {
        let mut rec = gpu.begin(Queue::Graphics);
        for _ in 0..10 {
            trace(&mut rec);
            serialize(&mut rec);
        }
        let point = gpu.submit(rec.finish(), &[]).expect("submit");
        traced.extend(ms(&gpu.resolve(point).expect("resolve"), "RT.Trace"));
    }
    traced.sort_by(f64::total_cmp);
    let median = traced[traced.len() / 2];
    eprintln!(
        "{}: BLAS {} tris {:.3} ms, TLAS 64 instances {:.3} ms, first trace {:.3} ms; {} traces of {}x{} rays: median {:.3} ms, p95 {:.3} ms ({:.0} Mrays/s)",
        gpu.info().name,
        indices.len() / 3,
        ms(&timings, "RT.BLAS")[0],
        ms(&timings, "RT.TLAS")[0],
        ms(&timings, "RT.Trace")[0],
        traced.len(),
        width,
        height,
        median,
        traced[traced.len() * 95 / 100],
        f64::from(width * height) / median / 1000.,
    );
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytemuck::cast_slice(bytes).to_vec()
}
