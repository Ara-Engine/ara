//! `trace()` timing on the R4-1/R5-6 fixtures (an ignored test: it is a benchmark).
//! Every set is rebuilt in-engine from the lab's raw inputs: the 183,752-triangle
//! E4 world (`vertices_a.bin`), the 108-triangle hot-spot asset and the crate, their
//! instance placements, and the lab's exact ray buffers. BLASes come from
//! `ara_bvh`, the TLAS and instance records from `look::trace`, the kernel is
//! `trace_bench` (`shaders/trace-test.slang`, workgroup 32). Each set is checked
//! against the lab's independent CPU references (4,096 sampled rays) before
//! 10 warm-ups and `ARA_TRACE_REPS` (default 500) individually timestamped
//! dispatches; nearest-rank p50/p99. Results: `target/ara-render/trace-bench/`.
//!
//! Run under the quiet-window lock:
//! `ARA_NO_GPU_VALIDATION=1 cargo test -p ara-render --release --lib trace_timing -- --ignored --nocapture`
//! (`ARA_TRACE_FIXTURES` overrides the lab directory).
use super::gpu::Harness;
use crate::look::trace::{NO_MATERIAL, TraceGpu, TraceInstance, TraceMesh, TraceScene};
use anyhow::{Context, Result, ensure};
use ara_bvh::{Blas, Mesh};
use ara_gpu::{Device, Resource};
use glam::{Affine3A, Vec3};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Extra instances of a set: their placements file and mesh.
type Extra<'a> = Option<(&'a str, &'a TraceMesh)>;

fn floats(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect())
}

/// A triangle soup of `f32x4` corners as a BLAS.
fn soup(path: &Path) -> Result<(TraceMesh, f64)> {
    let v = floats(path)?;
    let positions: Vec<[f32; 3]> = v
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let indices: Vec<u32> = (0..positions.len() as u32).collect();
    let start = Instant::now();
    let blas = Blas::build(&Mesh {
        positions: &positions,
        indices: &indices,
        submeshes: &[],
    })?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok((
        TraceMesh {
            blas: Arc::new(blas),
            uvs: None,
        },
        ms,
    ))
}

/// Translations of instances 1.. from a lab `instances_<set>.bin` (instance 0 is
/// the world).
fn placements(path: &Path) -> Result<Vec<Vec3>> {
    let v = floats(path)?;
    Ok(v.as_chunks::<4>()
        .0
        .iter()
        .skip(1)
        .map(|c| Vec3::new(c[0], c[1], c[2]))
        .collect())
}

fn put(scene: &mut TraceScene, id: u64, mesh: &TraceMesh, at: Vec3) -> Result<()> {
    scene.set_instance(
        id,
        TraceInstance {
            mesh: mesh.blas.hash,
            world: Affine3A::from_translation(at),
            mask: 1,
            materials: vec![NO_MATERIAL],
            user: id as u32,
        },
    )?;
    Ok(())
}

fn percentile(sorted: &[f64], p: usize) -> f64 {
    sorted[(sorted.len() * p).div_ceil(100) - 1]
}

#[test]
#[ignore = "trace() timing on the R4-1/R5-6 fixtures; run under the quiet-window lock"]
fn trace_timing() -> Result<()> {
    let dir = std::env::var("ARA_TRACE_FIXTURES")
        .unwrap_or_else(|_| "D:/AraBuild/render-research/lab/r5-6/r4-1".into());
    let dir = Path::new(&dir);
    let reps: usize = std::env::var("ARA_TRACE_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let only = std::env::var("ARA_TRACE_SETS").ok();
    let (mut world, world_ms) = soup(&dir.join("vertices_a.bin"))?;
    // A/B control: tinybvh's own tree for the world (the R4-1 build), same kernel.
    if std::env::var_os("ARA_TRACE_LAB_TREE").is_some() {
        let words = |f: &str| -> Result<Vec<[u32; 4]>> {
            let b = std::fs::read(dir.join(f))?;
            Ok(b.as_chunks::<16>()
                .0
                .iter()
                .map(|c| {
                    std::array::from_fn(|k| {
                        u32::from_le_bytes([c[k * 4], c[k * 4 + 1], c[k * 4 + 2], c[k * 4 + 3]])
                    })
                })
                .collect())
        };
        let mut blas = (*world.blas).clone();
        blas.nodes = words("nodes_a.bin")?;
        blas.triangles = words("tris_a.bin")?;
        world = TraceMesh {
            blas: Arc::new(blas),
            uvs: None,
        };
        eprintln!("world BLAS replaced by tinybvh's nodes_a/tris_a");
    }
    let (asset, asset_ms) = soup(&dir.join("vertices_asset.bin"))?;
    let (crate_mesh, _) = soup(&dir.join("vertices_crate.bin"))?;
    let s = world.blas.stats;
    eprintln!(
        "world BLAS: {} tris, {} refs, {} nodes, {} bytes, max stack {}, build {world_ms:.1} ms; asset build {asset_ms:.2} ms",
        s.triangles,
        s.references,
        s.nodes,
        world.blas.bytes(),
        s.max_stack
    );
    let mut h = Harness::new("trace-bench")?;
    // (set, rays/refs file, extra instances (placements, mesh), R5-6 4070S p50/p99)
    type Set<'a> = (&'a str, &'a str, Extra<'a>, Option<(f64, f64)>);
    let sets: [Set; 7] = [
        ("a", "a", None, Some((0.164, 0.169))),
        ("bp", "bp", None, Some((0.166, 0.168))),
        ("bp1440", "bp1440", None, Some((0.283, 0.285))),
        ("b1440", "b1440", None, Some((0.910, 1.359))),
        (
            "cp",
            "cp",
            Some(("instances_cp.bin", &asset)),
            Some((0.191, 0.198)),
        ),
        (
            "cp1440",
            "cp1440",
            Some(("instances_cp1440.bin", &asset)),
            Some((2.007, 2.500)),
        ),
        ("d", "d", Some(("instances_d.bin", &crate_mesh)), None),
    ];
    let mut rows = Vec::new();
    for (name, file, extra, sprint) in sets {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|s| s == name))
        {
            continue;
        }
        let mut scene = TraceScene::default();
        scene.add_mesh(world.clone());
        put(&mut scene, 0, &world, Vec3::ZERO)?;
        if let Some((placements_file, mesh)) = extra {
            scene.add_mesh(mesh.clone());
            for (i, at) in placements(&dir.join(placements_file))?
                .into_iter()
                .enumerate()
            {
                put(&mut scene, 1 + i as u64, mesh, at)?;
            }
        }
        let start = Instant::now();
        scene.commit();
        let tlas_ms = start.elapsed().as_secs_f64() * 1000.0;
        let trace = TraceGpu::new(&mut h.gpu, &scene)?;
        let mut raw = floats(&dir.join(format!("rays_{file}.bin")))?;
        for r in raw.as_chunks_mut::<8>().0 {
            r[3] = 0.0;
            r[7] = 1e30;
        }
        let rays: &[[f32; 8]] = raw.as_chunks::<8>().0;
        let n = rays.len() as u32;
        let (ray_buffer, ray_index) = h.rays(rays)?;
        let bytes = u64::from(n) * 16;
        let (out, out_index, readback) = h.results(bytes)?;
        // Correctness first: the lab's independent references.
        h.dispatch(
            h.bench,
            trace.record(),
            ray_index,
            (out, out_index),
            n,
            0,
            0xff,
            true,
        )?;
        let words: Vec<u32> = bytemuck::cast_slice(&h.read(out, readback, bytes)?).to_vec();
        let refs = std::fs::read(dir.join(format!("refs_{file}.bin")))?;
        let (mut mismatches, mut far, mut max_dt) = (0, 0, 0f32);
        let count = n as u64;
        for k in 0..4096u64 {
            let i = (k * count / 4096
                + u64::from((k as u32).wrapping_mul(2654435761)) % (count / 4096))
                as usize;
            let want = f32::from_le_bytes(refs[k as usize * 8..k as usize * 8 + 4].try_into()?);
            let got = f32::from_bits(words[i * 4]);
            if (got >= 0.0) != (want < 1e29) {
                mismatches += 1;
            } else if got >= 0.0 {
                let dt = (got - want).abs();
                max_dt = max_dt.max(dt);
                if dt > 1e-4f32.max(want * 1e-5) {
                    far += 1;
                }
            }
        }
        // Timing.
        let mut times = Vec::with_capacity(reps);
        let mut exp_times = Vec::new();
        for k in 0..reps + 10 {
            let (bench, exp) = (h.bench, h.exp);
            let mut run = |pipeline| {
                h.dispatch(
                    pipeline,
                    trace.record(),
                    ray_index,
                    (out, out_index),
                    n,
                    0,
                    0xff,
                    false,
                )
            };
            let ms = run(bench)?;
            let exp = exp.map(&mut run).transpose()?;
            if k >= 10 {
                times.push(ms);
                exp_times.extend(exp);
            }
        }
        if !exp_times.is_empty() {
            exp_times.sort_by(f64::total_cmp);
            eprintln!(
                "{name:7} exp p50 {:.3} ms  p99 {:.3} ms",
                percentile(&exp_times, 50),
                percentile(&exp_times, 99)
            );
        }
        times.sort_by(f64::total_cmp);
        let (p50, p99) = (percentile(&times, 50), percentile(&times, 99));
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let max = times[times.len() - 1];
        eprintln!(
            "{name:7} {n:>9} rays  p50 {p50:.3} ms  p99 {p99:.3} ms  max {max:.3}  ({:.0} Mrays/s)  refs: {mismatches} hit/miss, {far} t failures (max {max_dt:.2e})  sprint {sprint:?}",
            f64::from(n) / p50 / 1000.0
        );
        rows.push(serde_json::json!({
            "set": name, "rays": n, "p50_ms": p50, "p99_ms": p99, "mean_ms": mean, "max_ms": max,
            "samples": times.len(), "ref_mismatches": mismatches, "ref_t_failures": far,
            "ref_max_dt": max_dt, "tlas_build_ms": tlas_ms, "instances": scene.instance_slots().len(),
            "sprint_4070s_p50_p99": sprint, "blas_bytes": trace.blas_bytes(&scene),
        }));
        ensure!(
            mismatches == 0 && far == 0,
            "{name}: {mismatches} hit/miss and {far} distance mismatches"
        );
        for b in [ray_buffer, out, readback] {
            h.gpu.retire(Resource::Buffer(b))?;
        }
        trace.destroy(&mut h.gpu)?;
        h.gpu.collect()?;
    }
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/trace-bench");
    std::fs::create_dir_all(&out_dir)?;
    let info = h.gpu.info().name.clone();
    let report = serde_json::json!({
        "device": info, "reps": reps, "world_build_ms": world_ms, "world_stats": format!("{s:?}"),
        "world_bytes": world.blas.bytes(), "rows": rows,
    });
    std::fs::write(
        out_dir.join("result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
