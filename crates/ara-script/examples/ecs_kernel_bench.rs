//! Actual ScriptHost/CLR W1/W3 kernels against S8's frozen twelve-float contract.
//! --threads is the total runner ceiling: pool workers plus the calling thread.
//! Compile/spawn, every-row scalar validation, hashes and optional --dump are untimed.
//! Use matching prebuilt .NET fixtures and DOTNET_TieredCompilation=0 for comparisons.
//! `direct` emits native ISA math for this Rust target, with a scalar capability fallback.
//! `--kernel-module` keeps the math explicitly cold behind an editable Game wrapper.
#[path = "ecs_kernel_bench/direct.rs"]
mod direct;
#[path = "ecs_kernel_bench/reference.rs"]
mod reference;
#[path = "ecs_kernel_bench/simd.rs"]
mod simd;
#[path = "ecs_kernel_bench/source.rs"]
mod source;
#[path = "../tests/support/mod.rs"]
mod support;
#[path = "ecs_kernel_bench/topology.rs"]
mod topology;

use std::collections::HashSet;
use std::error::Error;
use std::io::{BufWriter, Write};
use std::time::Instant;

use bevy_ecs::entity::Entity;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::{Value, json};

fn option(name: &str) -> Option<String> {
    let args = std::env::args().collect::<Vec<_>>();
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn number(name: &str, default: usize) -> Result<usize, Box<dyn Error>> {
    Ok(match option(name) {
        Some(value) => value.parse()?,
        None => default,
    })
}

fn read_vector(live: &support::Live, entity: Entity, name: &str) -> [f32; 4] {
    let component = live
        .host
        .read_component(&live.world, entity, name)
        .expect("authoritative component remains present");
    let values = component["value"].as_array().expect("float4 JSON array");
    assert_eq!(values.len(), 4, "full float4 representation");
    std::array::from_fn(|axis| values[axis].as_f64().expect("finite float4 field") as f32)
}

fn assert_presence(live: &support::Live, entities: &[Entity]) {
    let ids = [
        "BenchStableId",
        "BenchPosition",
        "BenchVelocity",
        "BenchRotation",
    ]
    .map(|name| {
        live.host
            .component_id(name)
            .expect("registered module component")
    });
    let expected = entities.iter().copied().collect::<HashSet<_>>();
    assert_eq!(
        expected.len(),
        entities.len(),
        "spawned native handles are unique"
    );
    let mut observed = HashSet::new();
    for archetype in live.world.archetypes().iter() {
        if !archetype.entities().is_empty() && ids.iter().any(|id| archetype.contains(*id)) {
            assert!(
                ids.iter().all(|id| archetype.contains(*id)),
                "all four components must remain present together"
            );
            for row in archetype.entities() {
                assert!(
                    expected.contains(&row.id()),
                    "unexpected or recycled native entity"
                );
                assert!(
                    observed.insert(row.id()),
                    "native entity appears exactly once"
                );
            }
        }
    }
    assert_eq!(
        observed, expected,
        "exact native identities and component presence"
    );
}

fn validate(
    live: &support::Live,
    entities: &[Entity],
    workload: &str,
    steps: usize,
    dump: Option<String>,
) -> Result<Value, Box<dyn Error>> {
    assert_presence(live, entities);
    // Borrowed ECS reads finish before validation workers start; workers see only
    // this owned snapshot, never the World or host callbacks.
    let records = entities
        .iter()
        .enumerate()
        .map(|(id, &entity)| {
            let stable_id = live
                .host
                .read_component(&live.world, entity, "BenchStableId")
                .expect("stable ID");
            assert_eq!(
                stable_id["value"].as_u64(),
                Some(id as u64),
                "exact stable identity"
            );
            ["BenchPosition", "BenchVelocity", "BenchRotation"]
                .map(|name| read_vector(live, entity, name))
        })
        .collect::<Vec<_>>();
    let pool = ComputeTaskPool::get();
    let validation_runners = pool.thread_num() + 1;
    let chunk_rows = records.len().div_ceil(validation_runners);
    let checks = pool.scope(|scope| {
        for (batch, chunk) in records.chunks(chunk_rows).enumerate() {
            scope.spawn(async move {
                let (mut bounces, mut bounced_rows) = (0u64, 0usize);
                for (offset, actual) in chunk.iter().enumerate() {
                    let id = batch * chunk_rows + offset;
                    let mut expected = reference::initial(id as u32);
                    let mut bounced = false;
                    for _ in 0..steps {
                        if workload == "w1" { reference::transform(&mut expected); }
                        else if reference::particle(&mut expected) { bounces += 1; bounced = true; }
                    }
                    bounced_rows += usize::from(bounced);
                    for (field, (value, wanted)) in actual.iter().flatten().zip(expected.into_iter().flatten()).enumerate() {
                        assert_eq!(value.to_bits(), wanted.to_bits(), "{workload} ID {id} field {field}: every bit, including w/quaternion fields");
                    }
                }
                (bounces, bounced_rows)
            });
        }
    });
    let bounces = checks.iter().map(|check| check.0).sum::<u64>();
    let bounced_rows = checks.iter().map(|check| check.1).sum::<usize>();
    let mut output = dump
        .map(std::fs::File::create)
        .transpose()?
        .map(BufWriter::new);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in records.iter().flatten().flatten() {
        let bytes = value.to_le_bytes();
        for byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        if let Some(writer) = output.as_mut() {
            writer.write_all(&bytes)?;
        }
    }
    if let Some(mut writer) = output {
        writer.flush()?;
    }
    if workload == "w3" && entities.len() >= 64 && steps > 0 {
        assert!(
            bounced_rows > 0,
            "initial negative-height particles exercise actual bounces"
        );
    }
    Ok(
        json!({"every_field_bitwise_valid": true, "exact_ids_and_component_presence": true,
        "state_hash": format!("{hash:016x}"), "hash_algorithm": "fnv1a64",
        "state_bytes": entities.len() * 48, "layout": "stable-ID ascending, 12 little-endian f32: p.xyzw/v.xyzw/q.xyzw",
        "bounced_rows": bounced_rows, "bounces": bounces, "validation_runner_ceiling": validation_runners}),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let workload = option("--workload").unwrap_or_else(|| "w1".into());
    let mode = option("--mode").unwrap_or_else(|| "span".into());
    let rows = number("--rows", 1_000_003)?;
    let threads = number("--threads", 1)?;
    let warmup = number("--warmup", 24)?;
    let samples = number("--samples", 80)?;
    let hot = std::env::args().any(|arg| arg == "--hot");
    let kernel_module = std::env::args().any(|arg| arg == "--kernel-module");
    if !["w1", "w3"].contains(&workload.as_str())
        || !["span", "simd", "direct"].contains(&mode.as_str())
        || rows == 0
        || rows > u32::MAX as usize
        || threads == 0
        || samples == 0
    {
        return Err("--workload w1|w3 --mode span|simd|direct --rows 1..u32::MAX --threads TOTAL>=1 --warmup N --samples N>=1 [--hot] [--kernel-module] [--dump PATH]".into());
    }
    let steps = warmup.checked_add(samples).ok_or("tick count overflow")?;
    // Bevy's native executor supports zero pool workers; the caller drains its scope.
    let workers = threads - 1;
    let pool = ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(workers)
            .on_thread_spawn(ara_script::register_tick_thread)
            .build()
    });
    assert_eq!(
        pool.thread_num(),
        workers,
        "fresh process must honor requested pool+caller ceiling"
    );
    let mut live = topology::start(&workload, &mode, hot, kernel_module)?;
    let entities = (0..rows)
        .map(|id| {
            let [p, v, q] = reference::initial(id as u32);
            live.spawn(&[
                ("BenchStableId", json!({"value": id})),
                ("BenchPosition", json!({"value": p})),
                ("BenchVelocity", json!({"value": v})),
                ("BenchRotation", json!({"value": q})),
            ])
        })
        .collect::<Vec<_>>();
    eprintln!(
        "setup_complete workload={workload} mode={mode} rows={rows} hot={hot} pool_workers={workers} runner_ceiling={threads}; beginning warmup"
    );
    for _ in 0..warmup {
        live.run();
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        live.run();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    assert!(
        live.diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.is_error()),
        "{:?}",
        live.diagnostics
    );
    let validation = validate(&live, &entities, &workload, steps, option("--dump"))?;
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    let mut result = json!({"backend": "Ara ScriptHost/CoreCLR", "workload": workload, "mode": mode, "hot": hot,
        "rows": rows, "threads": threads, "worker_threads": workers, "runner_ceiling": workers + 1,
        "direct_compiled_backend": (mode == "direct").then(direct::backend),
        "warmup": warmup, "samples": samples, "steps": steps,
        "p50_ms": sorted[(samples * 50).div_ceil(100) - 1], "p95_ms": sorted[(samples * 95).div_ceil(100) - 1],
        "ms": timings, "sorted_ms": sorted, "validation": validation});
    if kernel_module {
        result["topology"] = json!({
            "kind": "game-wrapper/cold-kernel", "kernel_mode": mode,
            "modules": live.host.modules().into_iter().map(|module| json!({
                "name": module.name, "slot": module.slot, "hot": module.hot,
                "generation": module.generation, "systems": module.counts.0,
            })).collect::<Vec<_>>()
        });
    }
    println!("{result}");
    assert_eq!(live.finish(), 0, "host generations must not leak");
    Ok(())
}
