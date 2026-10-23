//! Interleaved identical-world C++/Rust comparison; use run.ps1 for load evidence.
#[path = "../tests/support/mod.rs"]
mod support;
#[path = "performance/workloads.rs"]
mod workloads;
use ara_physics::*;
use ara_physics_jolt::JoltWorld;
use std::time::Instant;
use support::*;

type BenchResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn persist(records: &[serde_json::Value], complete: bool) -> BenchResult<String> {
    let result = serde_json::json!({"schemaVersion":2,"runId":std::env::var("ARA_PHYSICS_BENCH_RUN_ID").ok(),"complete":complete,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"instrumented":cfg!(feature="profile"),"records":records});
    let text = ara_json::to_canonical_string(&ara_json::sort_keys(&result));
    if let Some(path) = std::env::var_os("ARA_PHYSICS_BENCH_OUTPUT") {
        std::fs::write(path, &text)?;
    }
    Ok(text)
}
fn load() -> serde_json::Value {
    #[cfg(windows)]
    {
        let command = "$cpu=Get-CimInstance Win32_Processor; $os=Get-CimInstance Win32_OperatingSystem; [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');cpu=$cpu.Name;logicalProcessors=$cpu.NumberOfLogicalProcessors;cpuPercent=$cpu.LoadPercentage;freeMemoryKiB=$os.FreePhysicalMemory;processCount=(Get-Process).Count} | ConvertTo-Json -Compress";
        if let Ok(output) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", command])
            .output()
            && let Ok(value) = serde_json::from_slice(&output.stdout)
        {
            return value;
        }
    }
    serde_json::Value::Null
}
fn distribution(values: &mut [f64]) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let p = |fraction: f64| {
        values[((values.len() as f64 * fraction).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    serde_json::json!({"p50":p(0.5),"p95":p(0.95),"mean":values.iter().sum::<f64>()/values.len() as f64})
}
fn config(preset: &str, count: usize, threads: u32) -> BenchResult<PhysicsConfig> {
    let mut c = match preset {
        "s6" => PhysicsConfig::default(),
        "game_default" => PhysicsConfig::game_default(),
        "stress" => PhysicsConfig::stress(),
        "deterministic_network" => PhysicsConfig::deterministic_network(),
        _ => return Err(format!("unknown preset {preset}").into()),
    };
    c.max_bodies = count as u32 + 16;
    c.max_pairs = (c.max_bodies * 16).max(4096);
    c.max_contacts = c.max_pairs;
    c.threads = threads;
    // Identical to S6's throughput fixture. Listener cost is a separate workload.
    c.event_capacity = 0;
    c.shared_job_pool = std::env::var("ARA_PHYSICS_BENCH_POOL").is_ok_and(|s| s == "shared");
    if let Ok(value) = std::env::var("ARA_PHYSICS_BENCH_SCRATCH_MB") {
        c.scratch_mb = value.parse()?;
    }
    if std::env::var("ARA_PHYSICS_BENCH_SPLITTER").is_ok_and(|s| s == "off") {
        c.large_island_splitter = false;
    }
    if std::env::var("ARA_PHYSICS_BENCH_ORDERING").is_ok_and(|s| s == "off") {
        c.deterministic_simulation = false;
    }
    Ok(c)
}
fn fixture(c: PhysicsConfig, count: usize) -> BenchResult<(JoltWorld, Vec<BodyHandle>)> {
    let mut w = JoltWorld::new(c)?;
    bowl(&mut w, Vec3::ZERO, 25.0, 12.0);
    let ids = grid(&mut w, count, Vec3::Y * 13.0, 50, 7);
    w.backend_mut().optimize()?;
    Ok((w, ids))
}
fn bowl_pair(
    preset: &str,
    count: usize,
    threads: u32,
    steps: usize,
    round: usize,
) -> BenchResult<serde_json::Value> {
    let load_before = load();
    let c = config(preset, count, threads)?;
    let mut native_config = c.clone();
    if let Ok(value) = std::env::var("ARA_PHYSICS_BENCH_NATIVE_SCRATCH_MB") {
        native_config.scratch_mb = value.parse()?;
    }
    if let Ok(value) = std::env::var("ARA_PHYSICS_BENCH_NATIVE_POOL") {
        native_config.shared_job_pool = value == "shared";
    }
    if let Ok(value) = std::env::var("ARA_PHYSICS_BENCH_NATIVE_SPLITTER") {
        native_config.large_island_splitter = value != "off";
    }
    if let Ok(value) = std::env::var("ARA_PHYSICS_BENCH_NATIVE_ORDERING") {
        native_config.deterministic_simulation = value != "off";
    }
    let same_settings = c == native_config;
    let start = Instant::now();
    let (mut ara, ids) = fixture(c.clone(), count)?;
    let setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let (mut native, native_ids) = fixture(native_config.clone(), count)?;
    let mut ara_ms = Vec::with_capacity(steps);
    let mut native_ms = Vec::with_capacity(steps);
    let mut inner_ms = Vec::with_capacity(steps);
    let mut extract_ms = Vec::with_capacity(steps);
    let mut column_ms = Vec::with_capacity(steps);
    let mut columns = BodyColumns::default();
    let mut native_columns = BodyColumns::default();
    let mut phases = [0u64; 8];
    let mut ara_phases = [0u64; 8];
    let mut scratch_peak = 0;
    let mut fallbacks = 0;
    let mut ara_scratch_peak = 0;
    let mut ara_fallbacks = 0;
    let mut states = Vec::with_capacity(count);
    let mut native_states = Vec::with_capacity(count);
    for tick in 0..steps {
        // AB/BA alternates every tick and round, so both see the same contention.
        for side in 0..2 {
            let is_ara = (side + tick + round).is_multiple_of(2);
            let world = if is_ara { &mut ara } else { &mut native };
            world.backend_mut().benchmark_reset();
            let start = Instant::now();
            if is_ara {
                world.step()?;
            } else {
                world.backend_mut().benchmark_native_step(DT)?;
            }
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let stats = world.backend().benchmark_stats();
            if is_ara {
                ara_ms.push(elapsed);
                for (total, value) in ara_phases.iter_mut().zip(stats) {
                    *total += value;
                }
                ara_scratch_peak = ara_scratch_peak.max(stats[9]);
                ara_fallbacks += stats[10];
            } else {
                native_ms.push(elapsed);
                inner_ms.push(stats[8] as f64 / 1e6);
                for (total, value) in phases.iter_mut().zip(stats) {
                    *total += value;
                }
                scratch_peak = scratch_peak.max(stats[9]);
                fallbacks += stats[10];
            }
        }
        // Warm both worlds identically. Reading only Ara after the pair would
        // give its next Update warmer body caches and bias the boundary delta.
        for world_side in 0..2 {
            let is_ara = (world_side + tick).is_multiple_of(2);
            let (world, handles, output, column_output) = if is_ara {
                (&mut ara, &ids, &mut states, &mut columns)
            } else {
                (
                    &mut native,
                    &native_ids,
                    &mut native_states,
                    &mut native_columns,
                )
            };
            for side in 0..2 {
                let aos = (side + tick).is_multiple_of(2);
                let start = Instant::now();
                if aos {
                    world.backend_mut().read_bodies(handles, output)?;
                } else {
                    world
                        .backend_mut()
                        .read_body_columns(handles, column_output)?;
                }
                if is_ara {
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    if aos {
                        extract_ms.push(elapsed);
                    } else {
                        column_ms.push(elapsed);
                    }
                }
            }
        }
    }
    let escaped = states
        .iter()
        .filter(|s| s.pose.position[1] < -1.0 || !s.pose.position.iter().all(|v| v.is_finite()))
        .count();
    if escaped != 0 {
        return Err(format!("{escaped} escaped bodies").into());
    }
    native
        .backend_mut()
        .read_bodies(&native_ids, &mut native_states)?;
    let identical_final_state = states == native_states
        && hash(&ara, &ids, &[], &[]) == hash(&native, &native_ids, &[], &[]);
    if same_settings && !identical_final_state {
        return Err("direct Jolt and Ara final states differ".into());
    }
    let ara_dist = distribution(&mut ara_ms);
    let native_dist = distribution(&mut native_ms);
    let ara_mean = ara_ms.iter().sum::<f64>() / steps as f64;
    let native_mean = native_ms.iter().sum::<f64>() / steps as f64;
    Ok(
        serde_json::json!({"scenario":"s6_bowl","preset":preset,"bodies":count,"threads":threads,"steps":steps,"round":round,
        "crossPlatformFp":ara.backend().deterministic(),"deterministicOrdering":c.deterministic_simulation,"velocitySteps":c.velocity_steps,"positionSteps":c.position_steps,
        "speculativeContactMeters":c.speculative_contact_distance,"sleepSeconds":c.time_before_sleep,"sleepSpeed":c.sleep_velocity_threshold,
        "splitter":c.large_island_splitter,"sharedPool":c.shared_job_pool,"contacts":false,"setupMs":setup_ms,
        "araMs":ara_dist,"nativeMs":native_dist,"nativeInnerMs":distribution(&mut inner_ms),"extractionMs":distribution(&mut extract_ms),
        "sameSettings":same_settings,"meanAraVsNativePercent":100.0*(ara_mean/native_mean-1.0),"phaseSummedWorkerElapsedMsPerTick":phases.map(|v|v as f64/steps as f64/1e6),
        "araPhaseSummedWorkerElapsedMsPerTick":ara_phases.map(|v|v as f64/steps as f64/1e6),
        "scratchMiB":c.scratch_mb,"nativeScratchMiB":native_config.scratch_mb,"nativeSharedPool":native_config.shared_job_pool,"nativeSplitter":native_config.large_island_splitter,"nativeDeterministicOrdering":native_config.deterministic_simulation,
        "columnExtractionMs":distribution(&mut column_ms),"loadBefore":load_before,"loadAfter":load(),
        "phaseOrder":["broadphase","narrowphase","constraint_setup","velocity_solver","position_solver","integration_ccd","islands","bookkeeping"],
        "scratchPeakBytes":scratch_peak,"scratchFallbackAllocations":fallbacks,"araScratchPeakBytes":ara_scratch_peak,"araScratchFallbackAllocations":ara_fallbacks,
        "escaped":escaped,"identicalFinalState":identical_final_state,"activeAtEnd":ara.backend().active_count()}),
    )
}
fn env_parse<T: std::str::FromStr>(key: &str, default: &str) -> BenchResult<T>
where
    T::Err: std::error::Error + 'static,
{
    Ok(std::env::var(key)
        .unwrap_or_else(|_| default.into())
        .parse()?)
}
fn main() -> BenchResult<()> {
    if let Some(dir) = std::env::var_os("ARA_PHYSICS_BENCH_AGGREGATE_DIR") {
        let mut cases = serde_json::Map::new();
        let run_id = std::env::var("ARA_PHYSICS_BENCH_RUN_ID")?;
        let manifest = std::env::var("ARA_PHYSICS_BENCH_CASES")?;
        let names = manifest.split(',').collect::<Vec<_>>();
        if names.is_empty() {
            return Err("empty case manifest".into());
        }
        for &name in &names {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                return Err("invalid case manifest name".into());
            }
            let path = std::path::Path::new(&dir).join(format!("{name}.json"));
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
            if value.get("complete").is_some_and(|v| v != true) {
                return Err(format!("incomplete case: {name}").into());
            }
            // Retained historical binaries predate run IDs; their fresh launches
            // are linked by the explicit manifest and provenance receipts.
            if value.get("runId").is_some_and(|v| v != &run_id) {
                return Err(format!("case belongs to a different run: {name}").into());
            }
            if cases.insert(name.into(), value).is_some() {
                return Err("duplicate case name".into());
            }
        }
        let result =
            serde_json::json!({"schemaVersion":2,"runId":run_id,"caseNames":names,"cases":cases});
        let output =
            std::env::var_os("ARA_PHYSICS_BENCH_OUTPUT").ok_or("aggregate output required")?;
        std::fs::write(
            output,
            ara_json::to_canonical_string(&ara_json::sort_keys(&result)),
        )?;
        return Ok(());
    }
    let steps = env_parse::<usize>("ARA_PHYSICS_BENCH_STEPS", "600")?;
    if steps < 2 {
        return Err("at least two ticks required".into());
    }
    let threads = env_parse::<u32>("ARA_PHYSICS_BENCH_THREADS", "8")?;
    let rounds = env_parse::<usize>("ARA_PHYSICS_BENCH_ROUNDS", "2")?;
    let counts = std::env::var("ARA_PHYSICS_BENCH_COUNTS")
        .unwrap_or_else(|_| "5000,20000".into())
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<usize>, _>>()?;
    let presets = std::env::var("ARA_PHYSICS_BENCH_PRESETS")
        .unwrap_or_else(|_| "s6,game_default,stress,deterministic_network".into());
    let mut records = Vec::new();
    for round in 0..rounds {
        for preset in presets.split(',').filter(|s| !s.is_empty()) {
            for &count in &counts {
                let record = bowl_pair(preset, count, threads, steps, round)?;
                eprintln!(
                    "{}",
                    ara_json::to_canonical_string(&ara_json::sort_keys(&record))
                );
                records.push(record);
                persist(&records, false)?;
            }
        }
    }
    if std::env::var("ARA_PHYSICS_BENCH_EXTRAS").is_ok_and(|s| s == "1") {
        records.extend(workloads::run(threads, steps)?);
    }
    let text = persist(&records, true)?;
    print!("{text}");
    Ok(())
}
