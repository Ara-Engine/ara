//! Real Rust-host/C# numeric query timing. Build the matching dotnet/ fixtures first;
//! use ARA_DOTNET_PREBUILT=1, DOTNET_TieredCompilation=0 and an optimized Rust profile.
//! --rows, --threads, --warmup and --samples configure one fresh-process measurement.
//! Allocation counters run only in separate, untimed --allocation-samples ticks.
#[path = "../tests/support/mod.rs"]
mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use ara_physics::{PhysicsPose, Velocity};
use bevy_ecs::prelude::Component;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::json;

struct Allocations;
static TRACK: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: all allocation operations are delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Allocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer/layout contract is forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        // SAFETY: the pointer/layout/new-size contract is forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocations = Allocations;

#[derive(Component)]
struct Dense;
#[derive(Component)]
#[component(storage = "SparseSet")]
struct Sparse;

const GAME: &str = r#"
/// <summary>One contiguous numeric kernel over actual native physics columns.</summary>
public static partial class Movement
{
    [Query]
    static void Step(ref Velocity velocity, ref PhysicsPose pose, in Tick tick)
    {
        velocity.Linear.X += 1;
        pose.Position.X += velocity.Linear.X * tick.Dt;
        pose.Position.Y += velocity.Linear.Y * tick.Dt;
        pose.Position.Z += velocity.Linear.Z * tick.Dt;
    }
}
"#;

fn arg(name: &str, default: usize) -> usize {
    let args = std::env::args().collect::<Vec<_>>();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let rows = arg("--rows", 1_000_000);
    let threads = arg("--threads", 15).max(1);
    let warmup = arg("--warmup", 24);
    let samples = arg("--samples", 80).max(1);
    let allocation_samples = arg("--allocation-samples", 8).max(1);
    let fragmented = std::env::args().any(|a| a == "--fragmented");
    let hot = std::env::args().any(|a| a == "--hot");
    ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(threads)
            .on_thread_spawn(ara_script::register_tick_thread)
            .build()
    });
    let root = support::Live::write("query-tick-bench", &[("Game", GAME, &[])]);
    let manifest = root.join("Game/ara.module.toml");
    let source = std::fs::read_to_string(&manifest).expect("benchmark module");
    if !hot {
        std::fs::write(manifest, source.replace("hot = true", "hot = false")).expect("cold module");
    }
    let mut live = support::Live::host(root);
    let spec = ara_script::ModuleSpec::from_dir(&live.root.join("Game")).expect("module spec");
    live.host
        .load_modules(&mut live.world, &[spec])
        .expect("compile cold kernel");
    let entities = (0..rows)
        .map(|i| {
            let mut entity = live.world.spawn((
                Velocity {
                    linear: [2.0, 3.0, 4.0],
                    angular: [-0.0, 6.0, 7.0],
                },
                PhysicsPose::default(),
            ));
            if fragmented && i.is_multiple_of(2) {
                entity.insert(Dense);
            }
            if fragmented && i.is_multiple_of(3) {
                entity.insert(Sparse);
            }
            entity.id()
        })
        .collect::<Vec<_>>();
    let added = live
        .world
        .entity(entities[0])
        .get_change_ticks::<Velocity>()
        .expect("ticks")
        .added;
    for _ in 0..warmup {
        live.run();
    }
    let mut timings = Vec::with_capacity(samples);
    // Latency samples exclude contended allocation-counter increments.
    TRACK.store(false, Ordering::Relaxed);
    for _ in 0..samples {
        let start = Instant::now();
        live.run();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    COUNT.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    for _ in 0..allocation_samples {
        live.run();
    }
    TRACK.store(false, Ordering::Relaxed);
    let (mut velocity, mut position) = ([2.0f32, 3.0, 4.0], [0.0f32; 3]);
    for _ in 0..warmup + samples + allocation_samples {
        velocity[0] += 1.0;
        for axis in 0..3 {
            position[axis] += velocity[axis] * (1.0f32 / 60.0);
        }
    }
    let final_ticks = live
        .world
        .entity(entities[0])
        .get_change_ticks::<Velocity>()
        .expect("final ticks");
    assert_eq!(
        final_ticks.added, added,
        "change stamping must not change added ticks"
    );
    let mut checksum = 0u64;
    for entity in entities {
        let value = live.world.get::<Velocity>(entity).expect("native velocity");
        let pose = live.world.get::<PhysicsPose>(entity).expect("native pose");
        assert_eq!(value.linear.map(f32::to_bits), velocity.map(f32::to_bits));
        assert_eq!(pose.position.map(f32::to_bits), position.map(f32::to_bits));
        assert_eq!(
            pose.rotation.0.map(f32::to_bits),
            PhysicsPose::default().rotation.0.map(f32::to_bits)
        );
        assert_eq!(
            value.angular.map(f32::to_bits),
            [-0.0f32, 6.0, 7.0].map(f32::to_bits)
        );
        let entity = live.world.entity(entity);
        assert_eq!(
            entity
                .get_change_ticks::<Velocity>()
                .expect("velocity tick")
                .changed,
            final_ticks.changed
        );
        assert_eq!(
            entity
                .get_change_ticks::<PhysicsPose>()
                .expect("pose tick")
                .changed,
            final_ticks.changed
        );
        for bits in value
            .linear
            .into_iter()
            .chain(pose.position)
            .map(f32::to_bits)
        {
            checksum = checksum.wrapping_add(u64::from(bits));
        }
    }
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    let output = json!({
        "rows":rows,"workers":ComputeTaskPool::get().thread_num(),"fragmented":fragmented,
        "rust_profile":"perf-opt3-lto-off-cgu256","csharp_hot":hot,"caller_participates":true,
        "warmup":warmup,"samples":samples,"p50_ms":sorted[samples/2],
        "allocation_counters_enabled_during_timing":false,
        "allocation_samples":allocation_samples,"allocation_probe":"separate-untimed-ticks-after-latency",
        "total_ticks":warmup+samples+allocation_samples,
        "p95_ms":sorted[(samples*95/100).min(samples-1)],"samples_ms":timings,
        "rust_allocations_per_tick":COUNT.load(Ordering::Relaxed) as f64/allocation_samples as f64,
        "rust_allocated_bytes_per_tick":BYTES.load(Ordering::Relaxed) as f64/allocation_samples as f64,
        "velocity_bits":velocity.map(f32::to_bits),"position_bits":position.map(f32::to_bits),
        "checksum":checksum,"changed_tick":final_ticks.changed.get(),
    });
    if let Some(path) = std::env::var_os("ARA_QUERY_BENCH_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&output).expect("measurement JSON"),
        )
        .expect("measurement output");
    }
    println!("{output}");
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0, "no leaked generations");
}
