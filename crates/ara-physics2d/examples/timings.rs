//! ECS-inclusive step, snapshot and restore timings; writes measurements/solver.json.
use anyhow::Result;
use ara_physics2d::*;
use ara_sim::{Identity, bevy_ecs::prelude::World};
use std::time::Instant;
fn main() -> Result<()> {
    let mut report = Vec::new();
    for count in [128, 1024] {
        let mut world = World::new();
        world.spawn((
            Identity("e_floor".into()),
            RigidBody2d::default(),
            Collider2d {
                half_extents: [100., 0.5],
                ..Default::default()
            },
            PhysicsPose2d {
                position: [0., -0.5],
                rotation: 0.,
            },
        ));
        for i in 0..count {
            world.spawn((
                Identity(format!("e_box{i:08}")),
                RigidBody2d {
                    motion: MotionType2d::Dynamic,
                    ..Default::default()
                },
                Collider2d::default(),
                PhysicsPose2d {
                    position: [(i % 64) as f32 * 1.1 - 32., (i / 64) as f32 * 1.1 + 0.5],
                    rotation: 0.,
                },
            ));
        }
        world.insert_resource(Physics2d::new(Physics2dConfig::default())?);
        let mut times = Vec::new();
        for i in 0..180 {
            let at = Instant::now();
            tick_ecs(&mut world)?;
            if i >= 60 {
                times.push(at.elapsed().as_secs_f64() * 1000.);
            }
        }
        times.sort_by(f64::total_cmp);
        let physics = world.resource::<Physics2d>();
        let at = Instant::now();
        let snapshot = physics.snapshot()?;
        let snapshot_ms = at.elapsed().as_secs_f64() * 1000.;
        let mut restored = Physics2d::new(Physics2dConfig::default())?;
        let at = Instant::now();
        restored.restore(&snapshot)?;
        let restore_ms = at.elapsed().as_secs_f64() * 1000.;
        report.push(serde_json::json!({"bodies":count+1,"step_p50_ms":times[times.len()/2],"step_p95_ms":times[times.len()*95/100],"snapshot_bytes":snapshot.len(),"snapshot_ms":snapshot_ms,"restore_ms":restore_ms,"samples":times.len()}));
    }
    let profile = if cfg!(debug_assertions) {
        "dev opt-level=1, dependencies=3"
    } else {
        "release"
    };
    let output = serde_json::json!({"backend":"Rapier2D enhanced-determinism, serial","profile":profile,"hz":60,"includes":"stable-ID ECS sync, validation, solver step and component writeback","results":report});
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("measurements");
    std::fs::create_dir_all(&dir)?;
    let name = if cfg!(debug_assertions) {
        "solver.json"
    } else {
        "solver-release.json"
    };
    std::fs::write(dir.join(name), ara_2d::canonical_json(&output))?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
