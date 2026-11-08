//! Compares identical per-row and span arithmetic, plus an explicit cross-row SIMD variant.
//! Each invocation is a fresh process: --mode row|batch|simd, --hot, --rows,
//! --threads, --warmup and --samples. Compile matching .NET fixtures first and use
//! ARA_DOTNET_PREBUILT=1 with DOTNET_TieredCompilation=0 for steady-state comparisons.
#[path = "batch_query_bench/allocations.rs"]
mod allocations;
#[path = "../tests/support/mod.rs"]
mod support;

use std::time::Instant;

use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::json;

#[global_allocator]
static ALLOCATOR: allocations::Allocator = allocations::Allocator;

const TYPES: &str = r#"
/// <summary>Contiguous authoritative scalar state.</summary>
[Component] public partial struct State
{
    /// <summary>Current value.</summary>
    public float Value;
}
/// <summary>Immutable per-row input.</summary>
[Component] public partial struct Rate
{
    /// <summary>Rate per second.</summary>
    public float Value;
}
"#;

const ROW: &str = r#"
    [Query] static void Run(ref State state, in Rate rate, in Tick tick)
    {
        var value = state.Value;
        var step = rate.Value * tick.Dt;
        for (var pass = 0; pass < 4; pass++) value = value * 0.99999f + step;
        state.Value = value;
    }
"#;

const BATCH: &str = r#"
    [Query] static void Run(Span<State> states, ReadOnlySpan<Rate> rates, in Tick tick)
    {
        for (var i = 0; i < states.Length; i++)
        {
            var value = states[i].Value;
            var step = rates[i].Value * tick.Dt;
            for (var pass = 0; pass < 4; pass++) value = value * 0.99999f + step;
            states[i].Value = value;
        }
    }
"#;

const SIMD: &str = r#"
    [Query] static void Run(Span<State> states, ReadOnlySpan<Rate> rates, in Tick tick)
    {
        var values = System.Runtime.InteropServices.MemoryMarshal.Cast<State, float>(states);
        var speeds = System.Runtime.InteropServices.MemoryMarshal.Cast<Rate, float>(rates);
        var scale = new System.Numerics.Vector<float>(0.99999f);
        var dt = new System.Numerics.Vector<float>(tick.Dt);
        var width = System.Numerics.Vector<float>.Count;
        var i = 0;
        for (; i <= values.Length - width; i += width)
        {
            var value = new System.Numerics.Vector<float>(values.Slice(i, width));
            var step = new System.Numerics.Vector<float>(speeds.Slice(i, width)) * dt;
            for (var pass = 0; pass < 4; pass++) value = value * scale + step;
            value.CopyTo(values.Slice(i, width));
        }
        for (; i < values.Length; i++)
        {
            var value = values[i];
            var step = speeds[i] * tick.Dt;
            for (var pass = 0; pass < 4; pass++) value = value * 0.99999f + step;
            values[i] = value;
        }
    }
"#;

fn option(name: &str) -> Option<String> {
    let args = std::env::args().collect::<Vec<_>>();
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn number(name: &str, default: usize) -> usize {
    option(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let rows = number("--rows", 1_000_003).max(1);
    let threads = number("--threads", 1).max(1);
    let warmup = number("--warmup", 24);
    let samples = number("--samples", 80).max(1);
    let allocation_samples = number("--allocation-samples", 8);
    let hot = std::env::args().any(|arg| arg == "--hot");
    let mode = option("--mode").unwrap_or_else(|| "row".into());
    let kernel = match mode.as_str() {
        "row" => ROW,
        "batch" => BATCH,
        "simd" => SIMD,
        _ => panic!("--mode must be row, batch or simd"),
    };
    ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(threads)
            .on_thread_spawn(ara_script::register_tick_thread)
            .build()
    });
    let source = format!(
        "{TYPES}\n/// <summary>Four identical multiply/add integration steps per tick.</summary>\npublic static partial class Integrate {{\n{kernel}\n}}"
    );
    let root = support::Live::write("batch-query-bench", &[("Game", &source, &[])]);
    if !hot {
        let manifest = root.join("Game/ara.module.toml");
        let contents = std::fs::read_to_string(&manifest).expect("module manifest");
        std::fs::write(manifest, contents.replace("hot = true", "hot = false"))
            .expect("cold module");
    }
    let mut live = support::Live::host(root);
    let module = ara_script::ModuleSpec::from_dir(&live.root.join("Game")).expect("module spec");
    live.host
        .load_modules(&mut live.world, &[module])
        .expect("compile kernel");
    let entities = (0..rows)
        .map(|index| {
            live.spawn(&[
                ("State", json!({"value": (index % 17) as f32 * 0.25})),
                ("Rate", json!({"value": (index % 13 + 1) as f32 * 0.5})),
            ])
        })
        .collect::<Vec<_>>();
    for _ in 0..warmup {
        live.run();
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        live.run();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (rust_allocations, rust_bytes) = allocations::measure(|| {
        for _ in 0..allocation_samples {
            live.run();
        }
    });
    assert!(
        live.diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.is_error()),
        "{:?}",
        live.diagnostics
    );
    let mut expected = [0.0f32; 17 * 13];
    for (index, value) in expected.iter_mut().enumerate() {
        *value = (index % 17) as f32 * 0.25;
        let step = (index % 13 + 1) as f32 * 0.5 * (1.0f32 / 60.0);
        for _ in 0..warmup + samples + allocation_samples {
            for _ in 0..4 {
                *value = *value * 0.99999 + step;
            }
        }
    }
    // Check every row, including vector tails and varied source values, outside timing.
    for (index, entity) in entities.into_iter().enumerate() {
        let state = live
            .host
            .read_component(&live.world, entity, "State")
            .expect("final state");
        let rate = live
            .host
            .read_component(&live.world, entity, "Rate")
            .expect("readonly rate");
        assert_eq!(
            (state["value"].as_f64().expect("state value") as f32).to_bits(),
            expected[index % expected.len()].to_bits(),
            "state row {index}"
        );
        assert_eq!(
            rate["value"].as_f64(),
            Some((index % 13 + 1) as f64 * 0.5),
            "readonly row {index}"
        );
    }
    let acquisition_ms = timings.clone();
    timings.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({
            "mode": mode, "hot": hot, "rows": rows, "worker_threads": threads,
            "warmup": warmup, "samples": samples,
        "allocation_samples": allocation_samples, "rust_allocations": rust_allocations, "rust_bytes": rust_bytes,
            "median_ms": timings[samples / 2], "p95_ms": timings[(samples * 95 / 100).min(samples - 1)],
        "timings_ms": acquisition_ms, "sorted_ms": timings,
            "all_rows_bitwise_valid": true,
        })
    );
    assert_eq!(live.finish(), 0);
}
