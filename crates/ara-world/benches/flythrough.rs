//! Reproducible 4 km × 4 km, 1M-entity cooked-world flythrough.
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, Session, SimRegistry, TickConfig};
use ara_world::*;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Coverage(BTreeSet<String>);
impl Interest for Coverage {
    fn ready(&mut self, cell: &str, _: CellKey, _: bool, _: &[HlodProxy]) {
        self.0.insert(cell.into());
    }
    fn removed(&mut self, _: &str, _: CellKey) {}
    fn visibility(&mut self, _: &str, _: bool) {}
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut schema = SchemaDb::new();
    register(&mut schema);
    ara_components::register(&mut schema);
    let registry = SimRegistry::new(schema.clone())?;
    let mut root = Document::new("world", AssetId::parse("wd_benchmark")?);
    root.insert_unchecked(
        RecordId::parse("w_grid")?,
        serde_json::from_value(serde_json::json!({"cellSize":250}))?,
    );
    root.insert_unchecked(RecordId::parse("l_art")?, serde_json::from_value(serde_json::json!({"name":"Art","loaded":true,"visible":true,"range":50,"graceTicks":8}))?);
    for z in 0..16 {
        for x in 0..16 {
            root.insert_unchecked(
                RecordId::parse(&format!("c_{}", z * 16 + x))?,
                serde_json::from_value(
                    serde_json::json!({"x":x,"z":z,"slot":0,"layer":"l_art","documents":[]}),
                )?,
            );
        }
    }
    let manifest = Manifest::read(&root)?;
    let directory = tempfile::tempdir()?;
    let hash = *blake3::hash(
        ara_json::to_canonical_string(&ara_json::sort_keys(&schema.reflection())).as_bytes(),
    )
    .as_bytes();
    let mut cooked_bytes = 0usize;
    for z in 0..16 {
        for x in 0..16 {
            let number = z * 16 + x;
            let count = 3906 + usize::from(number < 64);
            let mut entities = Vec::with_capacity(count);
            for local in 0..count {
                let identity = format!("e_{}", number * 4000 + local);
                let record = serde_json::from_value(
                    serde_json::json!({"Transform":{"position":[x as f32*250.0 + (local%62) as f32*4.0,0.0,z as f32*250.0+(local/62) as f32*3.9]}}),
                )?;
                entities.push(CellEntity { identity, record });
            }
            let blob = CellBlob {
                cell: format!("c_{number}"),
                key: CellKey {
                    x: x as i32,
                    z: z as i32,
                    slot: 0,
                },
                schema: hash,
                entities,
                proxies: Vec::new(),
            };
            let bytes = blob.encode()?;
            cooked_bytes += bytes.len();
            std::fs::write(directory.path().join(format!("c_{number}.aracell")), bytes)?;
        }
    }
    let empty = Document::new("scene", AssetId::parse("sc_streaming")?);
    let edit = EditWorld::new(registry.clone(), empty)?;
    let mut session = Session::local(
        edit,
        TickConfig::default(),
        1,
        ara_sim::input::ActionMap::default(),
    )?;
    let mut baseline = Vec::new();
    for frame in 0..2064 {
        let begin = Instant::now();
        session.step()?;
        if frame >= 64 {
            baseline.push(begin.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let reservation = 16 * 1024 * 1024;
    let config = StreamConfig {
        memory_bytes: reservation * 12,
        cell_reservation: reservation,
        lookahead_seconds: 0.5,
    };
    let pool = WorkerPool::new(
        Arc::new(CookedSource::new(directory.path())?),
        registry,
        2,
        reservation,
    )?;
    let mut streaming = Streaming::new(&manifest, pool, config)?;
    let budget = ActivationBudget {
        time: Duration::from_micros(450),
        max_entities: 64,
    };
    let mut samples = Vec::new();
    let mut nonempty = Vec::new();
    let mut boundaries = Vec::new();
    let mut peak_working_set = 0usize;
    let mut peak_private = 0usize;
    let mut peak_reserved = 0;
    let mut peak_retained = 0;
    let mut peak_entities = 0;
    let mut spawned = 0;
    let mut despawned = 0;
    let start = Instant::now();
    let mut tick = 0;
    let mut coverage = Coverage::default();
    // Visit every cell in a serpentine path; 200 frames/cell lets bounded activation
    // complete under worker contention. Revisit the route to exercise real reloading.
    for pass in 0..2 {
        for row in 0..16 {
            for column in 0..16 {
                let x = if row % 2 == 0 { column } else { 15 - column };
                let z = if pass == 0 { row } else { 15 - row };
                for frame in 0..200 {
                    tick += 1;
                    let offset = (frame as f32 + 0.5) / 200.0 * 250.0;
                    let source = StreamingSource {
                        position: [
                            x as f32 * 250.0 + if row % 2 == 0 { offset } else { 250.0 - offset },
                            0.0,
                            z as f32 * 250.0 + 125.0,
                        ],
                        velocity: [if row % 2 == 0 { 75.0 } else { -75.0 }, 0.0, 0.0],
                        range_scale: 1.0,
                    };
                    streaming.update(&manifest, &[source], tick, &mut coverage)?;
                    let mut stats = FrameStats::default();
                    let boundary_start = Instant::now();
                    session.step_with_streaming(|boundary| {
                        stats = streaming
                            .activate(boundary, budget, &mut coverage)
                            .map_err(|e| ara_sim::SimError(e.to_string()))?;
                        Ok(())
                    })?;
                    boundaries.push(boundary_start.elapsed().as_secs_f64() * 1000.0);
                    samples.push(stats.elapsed.as_secs_f64() * 1000.0);
                    if stats.spawned + stats.despawned > 0 {
                        nonempty.push(stats.elapsed.as_secs_f64() * 1000.0);
                    }
                    spawned += stats.spawned;
                    despawned += stats.despawned;
                    peak_reserved = peak_reserved.max(stats.reserved_bytes);
                    peak_retained = peak_retained.max(stats.retained_bytes);
                    peak_entities = peak_entities.max(stats.entities);
                    if tick % 256 == 0
                        && let Some((working, private)) = process_memory()
                    {
                        peak_working_set = peak_working_set.max(working);
                        peak_private = peak_private.max(private);
                    }
                    for cell in manifest.cells.keys() {
                        if let Some(error) = streaming.failure(cell) {
                            return Err(format!("{cell}: {error}").into());
                        }
                    }
                    // Accelerate the 60 Hz path; yield to workers outside measurement.
                    std::thread::sleep(Duration::from_micros(100));
                }
            }
        }
    }
    for _ in 0..2000 {
        tick += 1;
        streaming.update(&manifest, &[], tick, &mut coverage)?;
        let mut stats = FrameStats::default();
        let boundary_start = Instant::now();
        session.step_with_streaming(|boundary| {
            stats = streaming
                .activate(boundary, budget, &mut coverage)
                .map_err(|e| ara_sim::SimError(e.to_string()))?;
            Ok(())
        })?;
        boundaries.push(boundary_start.elapsed().as_secs_f64() * 1000.0);
        samples.push(stats.elapsed.as_secs_f64() * 1000.0);
        if stats.spawned + stats.despawned > 0 {
            nonempty.push(stats.elapsed.as_secs_f64() * 1000.0);
        }
        spawned += stats.spawned;
        despawned += stats.despawned;
        if streaming.reserved_bytes() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_micros(100));
    }
    samples.sort_by(f64::total_cmp);
    nonempty.sort_by(f64::total_cmp);
    baseline.sort_by(f64::total_cmp);
    boundaries.sort_by(f64::total_cmp);
    let percentile =
        |values: &[f64], p: f64| values[((values.len() - 1) as f64 * p).ceil() as usize];
    println!(
        "world_km2=16 authored_entities=1000000 cells=256 passes=2 frames={} elapsed_s={:.2} cooked_MiB={:.2}",
        samples.len(),
        start.elapsed().as_secs_f64(),
        cooked_bytes as f64 / 1048576.0
    );
    println!(
        "activation_all_ms p50={:.4} p95={:.4} p99={:.4} max={:.4}",
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        percentile(&samples, 0.99),
        samples[samples.len() - 1]
    );
    println!(
        "empty_boundary_p99_ms={:.4} streaming_boundary_p99_ms={:.4} sampled_working_set_MiB={:.2} sampled_private_MiB={:.2}",
        percentile(&baseline, 0.99),
        percentile(&boundaries, 0.99),
        peak_working_set as f64 / 1048576.0,
        peak_private as f64 / 1048576.0
    );
    println!(
        "activation_busy_ms frames={} p50={:.4} p95={:.4} p99={:.4} max={:.4}",
        nonempty.len(),
        percentile(&nonempty, 0.5),
        percentile(&nonempty, 0.95),
        percentile(&nonempty, 0.99),
        nonempty[nonempty.len() - 1]
    );
    println!(
        "spawned={spawned} despawned={despawned} peak_entities={peak_entities} peak_reserved_MiB={:.2} peak_retained_estimate_MiB={:.2} final_entities={} final_reserved={}",
        peak_reserved as f64 / 1048576.0,
        peak_retained as f64 / 1048576.0,
        streaming.entity_count(),
        streaming.reserved_bytes()
    );
    println!("fully_activated_unique_cells={}", coverage.0.len());
    if percentile(&nonempty, 0.99) >= 1.0
        || coverage.0.len() != 256
        || streaming.entity_count() != 0
        || streaming.reserved_bytes() != 0
        || spawned < 1_000_000
    {
        return Err("flythrough acceptance gate failed".into());
    }
    Ok(())
}

#[cfg(windows)]
fn process_memory() -> Option<(usize, usize)> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            bytes: u32,
        ) -> i32;
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        faults: 0,
        peak_working: 0,
        working: 0,
        peak_paged: 0,
        paged: 0,
        peak_nonpaged: 0,
        nonpaged: 0,
        pagefile: 0,
        peak_pagefile: 0,
        private: 0,
    };
    // SAFETY: the OS writes exactly the initialized repr(C) structure size; the
    // pseudo-handle denotes this process and remains valid for the call.
    let success = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<Counters>() as u32,
        )
    };
    (success != 0).then_some((counters.working, counters.private))
}
#[cfg(not(windows))]
fn process_memory() -> Option<(usize, usize)> {
    None
}
