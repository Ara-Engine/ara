//! S6 bowl through the complete ECS boundary, paired with direct native Update.
#[path = "../tests/support/mod.rs"]
mod support;
use ara_physics::*;
use ara_physics_jolt::{JoltBackend, JoltWorld};
use bevy_ecs::prelude::{Schedule, World};
use std::time::Instant;
use support::*;

type BenchResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn distribution(values: &mut [f64]) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        values[((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
    };
    serde_json::json!({"mean":values.iter().sum::<f64>()/values.len() as f64,
        "p50":percentile(0.5),"p95":percentile(0.95)})
}

fn main() -> BenchResult<()> {
    let count = 20000;
    let preset =
        std::env::var("ARA_PHYSICS_BENCH_PRESET").unwrap_or_else(|_| "game_default".into());
    let mut config = match preset.as_str() {
        "s6" => PhysicsConfig::default(),
        "game_default" => PhysicsConfig::game_default(),
        "stress" => PhysicsConfig::stress(),
        "deterministic_network" => PhysicsConfig::deterministic_network(),
        _ => return Err("unknown preset".into()),
    };
    config.max_bodies = count as u32 + 16;
    config.max_pairs = config.max_bodies * 16;
    config.max_contacts = config.max_pairs;
    config.threads = 8;
    // Match the listener-free solver fixtures. Subscription has its own experiment.
    config.event_capacity = 0;
    let steps = std::env::var("ARA_PHYSICS_BENCH_STEPS")
        .ok()
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(600);
    if steps < 2 {
        return Err("at least two ticks required".into());
    }
    let mut world = World::new();
    register::<JoltBackend>(&mut world, &mut Schedule::default(), config.clone())?;
    let descriptions = {
        let mut physics = world.resource_mut::<JoltWorld>();
        let shape = bowl_shape(&mut physics, Vec3::ZERO, 25.0, 12.0);
        let mut descriptions = vec![BodyDesc {
            shape,
            user_data: 0,
            pose: PhysicsPose::default(),
            body: RigidBody::default(),
            collider: Collider::default(),
            velocity: Velocity::default(),
        }];
        descriptions.extend(grid_descriptions(
            &mut physics,
            count,
            Vec3::Y * 13.0,
            50,
            7,
        ));
        descriptions
    };
    for (i, desc) in descriptions.into_iter().enumerate() {
        world.spawn((
            PhysicsId {
                value: i as u64 + 1,
            },
            desc.pose,
            desc.body,
            Collider {
                kind: ColliderKind::Cooked,
                ..desc.collider
            },
            CookedCollider(desc.shape),
            desc.velocity,
        ));
    }
    prepare_ecs::<JoltBackend>(&mut world)?;
    let mut reference = JoltWorld::new(config)?;
    bowl(&mut reference, Vec3::ZERO, 25.0, 12.0);
    let reference_ids = grid(&mut reference, count, Vec3::Y * 13.0, 50, 7);
    reference.backend_mut().optimize()?;
    let mut total = Vec::with_capacity(steps);
    let mut native = Vec::with_capacity(steps);
    let mut phases: [Vec<f64>; 6] = std::array::from_fn(|_| Vec::with_capacity(steps));
    let mut columns = BodyColumns::default();
    for tick in 0..steps {
        for side in 0..2 {
            let start = Instant::now();
            if (side + tick).is_multiple_of(2) {
                tick_ecs::<JoltBackend>(&mut world)?;
                let maintenance = Instant::now();
                world.clear_trackers();
                phases[5].push(maintenance.elapsed().as_secs_f64() * 1000.0);
                total.push(start.elapsed().as_secs_f64() * 1000.0);
                let t = world.resource::<PhysicsTimings>();
                for (times, ns) in phases.iter_mut().zip([
                    t.synchronize_ns,
                    t.inputs_ns,
                    t.step_ns,
                    t.extraction_ns,
                    t.writeback_ns,
                ]) {
                    times.push(ns as f64 / 1e6);
                }
            } else {
                reference.backend_mut().benchmark_native_step(DT)?;
                native.push(start.elapsed().as_secs_f64() * 1000.0);
                // Both paths read native body columns between successive updates.
                reference
                    .backend_mut()
                    .read_body_columns(&reference_ids, &mut columns)?;
            }
        }
    }
    let ids = (2..=count as u64 + 1)
        .map(|id| body_for_id(&world, id).ok_or("missing ECS body"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let physics = world.resource::<JoltWorld>();
    let final_hash = hash(physics, &ids, &[], &[]);
    if final_hash != hash(&reference, &reference_ids, &[], &[]) {
        return Err("ECS bowl final body bits differ from native reference".into());
    }
    let settings = physics.config();
    let record = serde_json::json!({"scenario":"ecs_s6_bowl","bodies":count,"threads":8,
        "steps":steps,"preset":preset,"velocitySteps":settings.velocity_steps,"positionSteps":settings.position_steps,
        "scratchMiB":settings.scratch_mb,"contacts":false,"sharedPool":false,
        "crossPlatformFp":physics.backend().deterministic(),"identicalFinalState":true,
        "totalIncludesTrackerMaintenance":true,"totalMs":distribution(&mut total),
        "nativeMs":distribution(&mut native),"phaseOrder":["lifecycle","inputs","step","extraction","writeback","tracker_maintenance"],
        "phaseMs":phases.iter_mut().map(|p|distribution(p)).collect::<Vec<_>>(),"finalStateHash":format!("{final_hash:016x}")});
    let artifact = serde_json::json!({"schemaVersion":2,"runId":std::env::var("ARA_PHYSICS_BENCH_RUN_ID").ok(),
        "complete":true,"records":[record]});
    let text = ara_json::to_canonical_string(&ara_json::sort_keys(&artifact));
    if let Some(path) = std::env::var_os("ARA_PHYSICS_BENCH_OUTPUT") {
        std::fs::write(path, &text)?;
    }
    print!("{text}");
    Ok(())
}
