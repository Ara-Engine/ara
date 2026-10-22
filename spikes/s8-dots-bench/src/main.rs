use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use s8_host::ecs::BenchWorld;
use serde::Serialize;
use std::{error::Error, io::Write, time::Instant};

#[derive(Serialize)]
struct ResultRow {
    setup: String,
    workload: String,
    count: usize,
    threads: usize,
    warmup: u32,
    frames: u32,
    p50_ms: f64,
    p95_ms: f64,
    ms: Vec<f64>,
    sums: [f64; 12],
    samples: Vec<[f32; 12]>,
}

/// One process per case keeps pool configuration and tiering independent.
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 8 {
        return Err(
            "usage: s8-dots-bench WORKLOAD COUNT THREADS WARMUP FRAMES RESULT_JSON DUMP_OR_DASH"
                .into(),
        );
    }
    let workload = &args[1];
    let n = args[2].parse()?;
    let threads = args[3].parse()?;
    let warmup: u32 = args[4].parse()?;
    let frames: u32 = args[5].parse()?;
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(threads).build());
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()?;
    let mut world = BenchWorld::new(n);
    let mut ms = Vec::with_capacity(frames as usize);
    for frame in 0..warmup + frames {
        let start = Instant::now();
        world.step(workload, threads > 1, frame);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if frame >= warmup {
            ms.push(elapsed);
        }
    }
    let records = world.records();
    if args[7] != "-" {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&args[7])?);
        for record in &records {
            for f in record {
                file.write_all(&f.to_le_bytes())?;
            }
        }
    }
    let mut sums = [0.0; 12];
    for r in &records {
        for j in 0..12 {
            sums[j] += f64::from(r[j]);
        }
    }
    let samples = (0..64).map(|i| records[i * (n - 1) / 63]).collect();
    let mut sorted = ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let row = ResultRow {
        setup: "rust".into(),
        workload: workload.to_owned(),
        count: n,
        threads,
        warmup,
        frames,
        p50_ms: sorted[(frames as usize * 50).div_ceil(100) - 1],
        p95_ms: sorted[(frames as usize * 95).div_ceil(100) - 1],
        ms,
        sums,
        samples,
    };
    std::fs::write(&args[6], serde_json::to_vec_pretty(&row)?)?;
    println!(
        "{} n={} threads={} p50={:.3} p95={:.3} ms",
        workload, n, threads, row.p50_ms, row.p95_ms
    );
    Ok(())
}
