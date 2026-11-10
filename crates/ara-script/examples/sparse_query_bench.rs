//! Real CLR sparse-query latency with exact native state readback. Use a matching
//! prebuilt SDK and optimized Rust profile. --interleaved adds filtered shared-table
//! rows and fragmented archetypes; the default is one aligned sparse/dense run.
#[path = "../tests/sparse_views/fixture.rs"]
mod fixture;
#[path = "../tests/support/mod.rs"]
mod support;

use ara_physics::{PhysicsPose, Velocity};
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use fixture::{DenseMarker, SparseMarker, SparseTransform};
use serde_json::json;
use std::time::Instant;

const GAME: &str = r#"
/// <summary>One numeric callback over real sparse and dense engine storage.</summary>
public static partial class Move
{
    [Query]
    static void Step(ref Transform transform, in Velocity velocity, ref PhysicsPose pose, in Tick tick)
    {
        transform.Position.X += velocity.Linear.X * tick.Dt;
        transform.Position.Y += velocity.Linear.Y * tick.Dt;
        transform.Position.Z += velocity.Linear.Z * tick.Dt;
        pose.Position = transform.Position;
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
    let rows = arg("--rows", 10_000).max(1);
    let threads = arg("--threads", 15).max(1);
    let warmup = arg("--warmup", 24);
    let samples = arg("--samples", 80).max(1);
    let interleaved = std::env::args().any(|a| a == "--interleaved");
    ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(threads)
            .on_thread_spawn(ara_script::register_tick_thread)
            .build()
    });
    let mut live = fixture::start("sparse-query-bench", GAME, false);
    let entities = (0..rows)
        .map(|i| {
            let mut entity = live.world.spawn((
                Velocity {
                    linear: [2.0, 3.0, 4.0],
                    angular: [-0.0, 6.0, 7.0],
                },
                PhysicsPose::default(),
            ));
            if !interleaved || i.is_multiple_of(2) {
                entity.insert(SparseTransform {
                    scale: [1.0; 3],
                    ..Default::default()
                });
            }
            if interleaved && i.is_multiple_of(3) {
                entity.insert(DenseMarker);
            }
            if interleaved && i.is_multiple_of(5) {
                entity.insert(SparseMarker);
            }
            let entity = entity.id();
            let tick = live
                .world
                .entity(entity)
                .get_change_ticks::<Velocity>()
                .expect("initial velocity tick")
                .changed;
            (entity, tick)
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
    let mut position = [0.0f32; 3];
    for _ in 0..warmup + samples {
        for (axis, velocity) in [2.0f32, 3.0, 4.0].into_iter().enumerate() {
            position[axis] += velocity * (1.0f32 / 60.0);
        }
    }
    let mut checksum = 0u64;
    let mut selected = 0;
    for (entity, tick) in entities {
        let velocity = live
            .world
            .get::<Velocity>(entity)
            .expect("native dense velocity");
        assert_eq!(velocity.linear, [2.0, 3.0, 4.0]);
        assert_eq!(
            velocity.angular.map(f32::to_bits),
            [-0.0f32, 6.0, 7.0].map(f32::to_bits)
        );
        assert_eq!(
            live.world
                .entity(entity)
                .get_change_ticks::<Velocity>()
                .expect("readonly tick")
                .changed,
            tick
        );
        let pose = live.world.get::<PhysicsPose>(entity).expect("dense pose");
        if let Some(transform) = live.world.get::<SparseTransform>(entity) {
            selected += 1;
            assert_eq!(
                transform.position.map(f32::to_bits),
                position.map(f32::to_bits)
            );
            assert_eq!(pose.position.map(f32::to_bits), position.map(f32::to_bits));
            let row = live.world.entity(entity);
            assert_eq!(
                row.get_change_ticks::<SparseTransform>()
                    .expect("sparse write tick")
                    .changed,
                row.get_change_ticks::<PhysicsPose>()
                    .expect("dense write tick")
                    .changed
            );
            for bits in transform.position.map(f32::to_bits) {
                checksum = checksum.wrapping_add(u64::from(bits));
            }
        } else {
            assert_eq!(pose.position, [0.0; 3]);
            assert_eq!(
                live.world
                    .entity(entity)
                    .get_change_ticks::<PhysicsPose>()
                    .expect("unselected tick")
                    .changed,
                tick
            );
        }
    }
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    let output = json!({"rows":rows,"selected":selected,"workers":ComputeTaskPool::get().thread_num(),
        "caller_participates":true,"managed_threads_seen":live.host.diag(ara_script::abi::DIAG_THREADS_SEEN),
        "interleaved":interleaved,"csharp_hot":false,"allocation_counters_during_timing":false,
        "warmup":warmup,"samples":samples,"p50_ms":sorted[samples/2],
        "p95_ms":sorted[(samples*95/100).min(samples-1)],"samples_ms":timings,
        "position_bits":position.map(f32::to_bits),"checksum":checksum});
    if let Some(path) = std::env::var_os("ARA_SPARSE_BENCH_OUTPUT") {
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
    assert_eq!(live.finish(), 0);
}
