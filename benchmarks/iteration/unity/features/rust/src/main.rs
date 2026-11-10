//! Standalone retained-query fixture; build/run only in a granted CPU slot.
mod protocol;
mod world;

use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use std::{
    error::Error,
    io::Write,
    path::{Component, Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn target_path(raw: &str) -> Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .ok_or("fixture is not at its documented repository path")?;
    let allowed = root.join("target").canonicalize()?;
    let raw = Path::new(raw);
    let path = if raw.is_absolute() {
        raw.to_owned()
    } else {
        std::env::current_dir()?.join(raw)
    };
    if path.components().any(|part| part == Component::ParentDir) || !path.starts_with(&allowed) {
        return Err(
            "fixture output must stay under this checkout's target/ without parent traversals"
                .into(),
        );
    }
    let parent = path.parent().ok_or("output has no parent")?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or("output has no existing ancestor")?;
    }
    if !existing.canonicalize()?.starts_with(&allowed) {
        return Err("output directory symlink escapes target/".into());
    }
    std::fs::create_dir_all(parent)?;
    if path.exists() {
        return Err("output exists; choose fresh paths".into());
    }
    Ok(path)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 10 || args[1] != "query_filter" {
        return Err("usage: fixture query_filter COUNT WORKERS WARMUP FRAMES RESULT_JSON DUMP ARCHETYPES BATCH (WORKERS=0 for serial, >=2 for parallel)".into());
    }
    let count: u32 = args[2].parse()?;
    let workers: usize = args[3].parse()?;
    let warmup: u32 = args[4].parse()?;
    let frames: u32 = args[5].parse()?;
    let archetypes: u32 = args[8].parse()?;
    let batch: usize = args[9].parse()?;
    let steps = warmup.checked_add(frames).ok_or("step count overflow")?;
    if count < 64
        || frames == 0
        || workers == 1
        || workers > 63
        || ![8, 64].contains(&archetypes)
        || batch == 0
    {
        return Err("invalid feature fixture parameters".into());
    }
    let result = target_path(&args[6])?;
    let dump = target_path(&args[7])?;
    if result == dump {
        return Err("result and dump paths must differ".into());
    }
    if workers > 0 {
        ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(workers).build());
    }
    let mut fixture = world::Fixture::new(count, archetypes);
    let matched = fixture.matched_count();
    let expected = (0..count).filter(|id| *id & 1 != 0 && *id & 2 == 0).count();
    if matched != expected {
        return Err("retained query selected the wrong entity count".into());
    }
    let mut samples = Vec::with_capacity(frames as usize);
    for tick in 0..steps {
        let started = Instant::now();
        fixture.step(tick, workers > 0, batch);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if tick >= warmup {
            samples.push(elapsed);
        }
    }
    let records = fixture.records(steps)?;
    let (state_bytes, state_hash) = protocol::write(&dump, &records, steps)?;
    let mut ordered = samples.clone();
    ordered.sort_by(f64::total_cmp);
    let p50 = ordered[(frames as usize * 50).div_ceil(100) - 1];
    let p95 = ordered[(frames as usize * 95).div_ceil(100) - 1];
    let row = serde_json::json!({
        "lane": "bevy-retained-query-native", "workload": "query_filter", "count": count,
        "archetypes": archetypes, "matched_entities": matched, "configured_pool_workers": workers,
        "max_kernel_runners": if workers == 0 { 1 } else { workers + 1 },
        "caller_joins_jobs": true, "batch": batch, "warmup": warmup, "frames": frames,
        "logical_cpus": std::thread::available_parallelism()?.get(),
        "p50_ms": p50, "p95_ms": p95, "ms": samples,
        "full_ids_tags_updates_checked": true, "state_bytes": state_bytes,
        "state_blake3": state_hash, "component_bytes": {"stable_id": 4, "value_updates": 8}
    });
    let text = ara_json::to_canonical_string(&row);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(result)?
        .write_all(text.as_bytes())?;
    println!("{text}");
    Ok(())
}
