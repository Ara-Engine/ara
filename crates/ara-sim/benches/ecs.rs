//! Matched S8 workloads, with a baseline and tuned safe contiguous implementation.
#[path = "support/baseline.rs"]
mod baseline;
#[path = "support/kernels.rs"]
mod kernels;
#[path = "support/model.rs"]
mod model;
#[path = "support/sorted.rs"]
mod sorted;
#[path = "support/tuned.rs"]
mod tuned;

use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde::Serialize;
use std::{error::Error, time::Instant};

#[derive(Serialize)]
struct Row {
    workload: String,
    variant: String,
    count: usize,
    threads: usize,
    batch: usize,
    warmup: u32,
    frames: u32,
    p50_ms: f64,
    p95_ms: f64,
    ms: Vec<f64>,
    state_hash: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 9 {
        return Err("usage: ecs WORKLOAD COUNT THREADS WARMUP FRAMES baseline|tuned BATCH RESULT_JSON (or validate COUNT THREADS STEPS 1 tuned 1024 -)".into());
    }
    let workload = &args[1];
    let n: usize = args[2].parse()?;
    let threads = args[3].parse()?;
    let warmup: u32 = args[4].parse()?;
    let frames: u32 = args[5].parse()?;
    let variant = &args[6];
    let batch: usize = args[7].parse()?;
    if threads == 0
        || batch == 0
        || frames == 0
        || n < 64
        || !["baseline", "tuned", "simd"].contains(&variant.as_str())
    {
        return Err("invalid benchmark parameters".into());
    }
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(threads).build());
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()?;
    if workload == "profile-w2" {
        let mut world = baseline::BenchWorld::new(n);
        let mut stages = Vec::with_capacity(frames as usize);
        for frame in 0..warmup + frames {
            let row = world.profile_boids(threads > 1, batch);
            if frame >= warmup {
                stages.push(row);
            }
        }
        let medians: [f64; 3] = std::array::from_fn(|column| {
            let mut values: Vec<_> = stages.iter().map(|row| row[column]).collect();
            values.sort_by(f64::total_cmp);
            values[(frames as usize * 50).div_ceil(100) - 1]
        });
        let mut hash = blake3::Hasher::new();
        for row in world.records() {
            for value in row {
                hash.update(&value.to_le_bytes());
            }
        }
        let value = serde_json::json!({"workload":"w2", "count":n,"threads":threads,"batch":batch,"warmup":warmup,"frames":frames,
            "snapshot_p50_ms":medians[0],"grid_p50_ms":medians[1],"dispatch_p50_ms":medians[2],"stages_ms":stages,"state_hash":hash.finalize().to_hex().to_string()});
        if args[8] != "-" {
            std::fs::write(&args[8], ara_json::to_canonical_string(&value))?;
        }
        println!(
            "W2 {n} t{threads}: snapshot={:.3} grid={:.3} neighbour kernel/dispatch={:.3} ms",
            medians[0], medians[1], medians[2]
        );
        return Ok(());
    }
    if workload == "validate" || workload.starts_with("validate-") {
        for w in ["w1", "w2", "w3", "w4"] {
            if let Some(selected) = workload.strip_prefix("validate-")
                && selected != w
            {
                continue;
            }
            let count = if w == "w4" { 100_000 } else { n };
            let mut before = baseline::BenchWorld::new(count);
            let mut after = baseline::BenchWorld::new(count);
            let mut simd = baseline::BenchWorld::new(count);
            for frame in 0..warmup {
                before.step(w, threads > 1, frame);
                after.step_tuned(w, threads > 1, frame, batch);
                simd.step_simd(w, threads > 1, frame, batch);
            }
            let a = before.records();
            let b = after.records();
            let c = simd.records();
            if !a
                .iter()
                .zip(&b)
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits()))
            {
                return Err(format!("{w} result mismatch").into());
            }
            if !a
                .iter()
                .zip(&c)
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits()))
            {
                return Err(format!("{w} SIMD result mismatch").into());
            }
            println!("{w}: {count} entities bit-identical after {warmup} steps");
        }
        return Ok(());
    }
    if !["w1", "w2", "w3", "w4"].contains(&workload.as_str()) || (workload == "w4" && n != 100_000)
    {
        return Err("invalid workload/count".into());
    }
    let mut world = baseline::BenchWorld::new(n);
    let mut ms = Vec::with_capacity(frames as usize);
    for frame in 0..warmup + frames {
        let start = Instant::now();
        if variant == "baseline" {
            world.step(workload, threads > 1, frame);
        } else if variant == "simd" {
            world.step_simd(workload, threads > 1, frame, batch);
        } else {
            world.step_tuned(workload, threads > 1, frame, batch);
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if frame >= warmup {
            ms.push(elapsed);
        }
    }
    let mut hash = blake3::Hasher::new();
    for row in world.records() {
        for value in row {
            hash.update(&value.to_le_bytes());
        }
    }
    let mut sorted = ms.clone();
    sorted.sort_by(f64::total_cmp);
    let row = Row {
        workload: workload.clone(),
        variant: variant.clone(),
        count: n,
        threads,
        batch,
        warmup,
        frames,
        p50_ms: sorted[(frames as usize * 50).div_ceil(100) - 1],
        p95_ms: sorted[(frames as usize * 95).div_ceil(100) - 1],
        ms,
        state_hash: hash.finalize().to_hex().to_string(),
    };
    if args[8] != "-" {
        std::fs::write(
            &args[8],
            ara_json::to_canonical_string(&serde_json::to_value(&row)?),
        )?;
    }
    println!(
        "{} {} n={} threads={} batch={} p50={:.3} p95={:.3} ms",
        workload, variant, n, threads, batch, row.p50_ms, row.p95_ms
    );
    Ok(())
}
