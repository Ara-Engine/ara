//! Explicit opt-in dependency-plan and actual Bevy schedule construction timings.
use super::super::Dependencies;
use super::{declaration, legacy, legacy_conflict, planned};
use crate::host::ScriptSet;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{MultiThreadedExecutor, SingleThreadedExecutor};
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::json;
use std::hint::black_box;
use std::time::Instant;

#[derive(Resource, Default)]
struct Counter(u64);

fn percentile(values: &[f64], percent: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

fn systems(count: usize) -> Vec<super::SystemDecl> {
    (0..count)
        .map(|index| declaration(index, vec![], vec![(0, true)], false))
        .collect()
}

/// Measures plan allocation with alternating order; this excludes Bevy and CLR.
fn plan_times(count: usize) -> serde_json::Value {
    let systems = systems(count);
    let mut samples = [Vec::new(), Vec::new()];
    let mut edges = [0usize; 2];
    for round in 0..24 {
        for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = Instant::now();
            let graph = black_box(if variant == 0 {
                legacy(&systems)
            } else {
                planned(&systems).0
            });
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            edges[variant] = graph.iter().map(Vec::len).sum();
            if round >= 4 {
                samples[variant].push(elapsed);
            }
        }
    }
    assert_eq!(edges, [count * (count - 1) / 2, count - 1]);
    json!({"systems":count,"edges":edges,"ms":samples,
        "legacy_p50_ms":percentile(&samples[0],50),"planned_p50_ms":percentile(&samples[1],50),
        "legacy_p95_ms":percentile(&samples[0],95),"planned_p95_ms":percentile(&samples[1],95)})
}

/// Builds the same ordered stateful jobs with each graph/executor policy.
fn schedule_times(count: usize) -> serde_json::Value {
    let systems = systems(count);
    let mut samples = [Vec::new(), Vec::new()];
    for round in 0..8 {
        for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let mut world = World::new();
            world.insert_resource(Counter::default());
            let start = Instant::now();
            let mut dependencies = Dependencies::default();
            let mut schedule = Schedule::default();
            for (index, system) in systems.iter().enumerate() {
                let id = system.id;
                let mut config = (move |mut counter: ResMut<Counter>| {
                    counter.0 = counter.0.wrapping_mul(16_777_619) ^ u64::from(id);
                })
                .in_set(ScriptSet(id))
                .into_configs();
                if variant == 0 {
                    // Match the original lazy all-pairs loop, without allocating
                    // an extra legacy adjacency list before Bevy's own configs.
                    for earlier in systems[..index]
                        .iter()
                        .filter(|earlier| legacy_conflict(earlier, system))
                    {
                        config = config.after(ScriptSet(earlier.id));
                    }
                } else {
                    for earlier in dependencies.push(system) {
                        config = config.after(ScriptSet(earlier));
                    }
                }
                schedule.add_systems(config);
            }
            if variant != 0 && dependencies.is_chain() {
                schedule.set_executor(SingleThreadedExecutor::new());
            } else {
                schedule.set_executor(MultiThreadedExecutor::new());
            }
            schedule.initialize(&mut world).expect("valid ordered jobs");
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if round >= 2 {
                samples[variant].push(elapsed);
            }
            let mut expected = 0u64;
            for _ in 0..3 {
                schedule.run(&mut world);
                for system in &systems {
                    expected = expected.wrapping_mul(16_777_619) ^ u64::from(system.id);
                }
                assert_eq!(world.resource::<Counter>().0, expected);
            }
        }
    }
    json!({"systems":count,"build_ms":samples,"three_ticks_exact":true,
        "legacy_p50_ms":percentile(&samples[0],50),"planned_p50_ms":percentile(&samples[1],50),
        "legacy_p95_ms":percentile(&samples[0],95),"planned_p95_ms":percentile(&samples[1],95)})
}

#[test]
#[ignore = "explicit performance probe; run alone and capture under target"]
fn dependency_plan_benchmark() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(2).build());
    let plan = [128, 512, 2048].map(plan_times);
    let schedule = [64, 256, 512].map(schedule_times);
    println!(
        "{}",
        json!({"scope":"dependency planning and Bevy schedule initialization only; no CLR, compilation, scene load or complete play latency", "plan":plan,"schedule":schedule})
    );
}
