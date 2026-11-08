//! Five extended fixtures; the frozen query_filter binary remains independent.
mod buffers;
mod churn;
mod dag;
mod dirty;
mod export;
mod model;
mod probe;

use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use model::{Case, Frame};
use std::{
    error::Error,
    io::Write,
    path::{Component, Path, PathBuf},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn output(raw: &str) -> Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .ok_or("fixture repository path")?
        .join("target")
        .canonicalize()?;
    let path = std::path::absolute(raw)?;
    if path.components().any(|part| part == Component::ParentDir) || !path.starts_with(&root) {
        return Err("output must stay under checkout target/".into());
    }
    let parent = path.parent().ok_or("output parent")?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or("output ancestor")?;
    }
    if !existing.canonicalize()?.starts_with(&root) || path.exists() {
        return Err("unsafe or existing output".into());
    }
    std::fs::create_dir_all(parent)?;
    Ok(path)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 10 {
        return Err(
            "usage: extended CASE COUNT WORKERS WARMUP FRAMES RESULT DUMP CONFIG_JSON BATCH".into(),
        );
    }
    let case = Case::parse(&args[1]).ok_or("unknown extended case")?;
    let count: u32 = args[2].parse()?;
    let workers: usize = args[3].parse()?;
    let warmup: u32 = args[4].parse()?;
    let frames: u32 = args[5].parse()?;
    let batch: usize = args[9].parse()?;
    let steps = warmup.checked_add(frames).ok_or("step overflow")?;
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[8])?)?;
    if !matches!(
        config["distribution"].as_str(),
        Some("spread" | "clustered")
    ) {
        return Err("invalid distribution".into());
    }
    let frame = Frame {
        tick: 0,
        count,
        percent: config["percent"].as_u64().ok_or("percent")?.try_into()?,
        clustered: config["distribution"].as_str().ok_or("distribution")? == "clustered",
        length: config["length"].as_u64().ok_or("length")?.try_into()?,
        trim: config["trim"].as_bool().ok_or("trim")?,
        fork_join: config["fork_join"].as_bool().ok_or("fork_join")?,
        parallel: workers > 0,
    };
    if count < 64
        || frames == 0
        || workers == 1
        || workers > 63
        || batch == 0
        || frame.percent > 100
        || ![0, 4, 16, 64].contains(&frame.length)
    {
        return Err("invalid parameters".into());
    }
    let result = output(&args[6])?;
    let dump = output(&args[7])?;
    if result == dump {
        return Err("result and dump differ".into());
    }
    if workers > 0 {
        ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(workers).build());
    }
    let (mut world, entities) = model::setup(case, frame);
    let mut schedule = match case {
        Case::Dirty => Some(dirty::schedule(frame.parallel)),
        Case::Dag => Some(dag::schedule(frame.parallel, frame.fork_join)),
        _ => None,
    };
    let mut churn = (case == Case::Churn)
        .then(|| churn::Churn::new(&mut world, &entities, frame.percent, batch));
    let mut buffers =
        matches!(case, Case::Fixed | Case::Resize).then(|| buffers::Buffers::new(&mut world));
    let mut samples = Vec::with_capacity(frames as usize);
    for tick in 0..steps {
        let current = Frame { tick, ..frame };
        let started = Instant::now();
        *world.resource_mut::<Frame>() = current;
        if let Some(schedule) = &mut schedule {
            schedule.run(&mut world);
        }
        if let Some(churn) = &mut churn {
            world.increment_change_tick();
            churn.step(&mut world, current);
        }
        if let Some(buffers) = &mut buffers {
            world.increment_change_tick();
            buffers.step(&mut world, case, current);
        }
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if tick >= warmup {
            samples.push(elapsed);
        }
    }
    let (state_bytes, state_hash, layout) = export::write(&mut world, case, frame, steps, &dump)?;
    let diagnostics = if case == Case::Dirty {
        let (mut replay, _) = model::setup(
            case,
            Frame {
                parallel: false,
                ..frame
            },
        );
        replay.insert_resource(dirty::Counts::default());
        let mut observer = dirty::diagnostic_schedule();
        for tick in 0..steps {
            replay.resource_mut::<Frame>().tick = tick;
            observer.run(&mut replay);
        }
        let replay_path = PathBuf::from(format!("{}.diagnostic.bin", dump.display()));
        let (_, digest, _) = export::write(&mut replay, case, frame, steps, &replay_path)?;
        if digest != state_hash {
            return Err("diagnostic replay differs from timed state".into());
        }
        serde_json::json!({ "candidate_logical_visits": replay.resource::<dirty::Counts>().0,
            "separate_replay": true, "state_blake3": digest,
            "counter_scope": "returned consumer candidates, not total rows scanned by filter/marker" })
    } else {
        serde_json::Value::Null
    };
    let worker_probe = probe::observe(frame, case.schedule());
    let mut ordered = samples.clone();
    ordered.sort_by(f64::total_cmp);
    let scheduled = case.schedule();
    let row = serde_json::json!({ "lane": "bevy-native-extended", "workload": args[1], "count": count,
        "archetypes": 0, "configured_pool_workers": workers,
        "max_kernel_runners": if workers == 0 { 1 } else if scheduled { workers } else { workers + 1 },
        "caller_joins_jobs": if workers == 0 { true } else { !scheduled },
        "worker_semantics": if scheduled { "Send systems scope_with_executor(false); participation observed separately" } else { "scope/par_iter caller may join shared pool" },
        "worker_probe": worker_probe, "dirty_diagnostics": diagnostics,
        "batch": batch, "warmup": warmup, "frames": frames, "config": config,
        "logical_cpus": std::thread::available_parallelism()?.get(), "ms": samples,
        "p50_ms": ordered[(frames as usize * 50).div_ceil(100) - 1], "p95_ms": ordered[(frames as usize * 95).div_ceil(100) - 1],
        "full_ids_tags_updates_checked": true, "state_bytes": state_bytes, "state_blake3": state_hash,
        "layout": layout,
        "batch_scope": "explicit churn command-recording chunks only; retained Bevy par_iter uses its own default strategy",
        "expected_structural_commands": if case == Case::Churn { (0..count).filter(|id| id % 100 < frame.percent).count() as u64 * u64::from(steps) } else { 0 },
        "structural_count_basis": "selected stable entities times steps; state checks verify resulting presence and updates" });
    let text = ara_json::to_canonical_string(&row);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(result)?
        .write_all(text.as_bytes())?;
    println!("{text}");
    Ok(())
}
