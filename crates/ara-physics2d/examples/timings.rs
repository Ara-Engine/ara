use anyhow::Result;
use ara_physics2d::{Body2d, BodyKind2d, Collider2d, Physics2d};
use ara_sim::{Identity, bevy_ecs::prelude::World};
use std::time::Instant;
fn main() -> Result<()> {
    let mut report = Vec::new();
    for count in [128, 1024] {
        let mut world = World::new();
        world.spawn((
            Identity("e_floor".into()),
            Body2d {
                position: [0., -0.5],
                ..Default::default()
            },
            Collider2d {
                half_extents: [100., 0.5],
                ..Default::default()
            },
        ));
        for i in 0..count {
            world.spawn((
                Identity(format!("e_box{i:08}")),
                Body2d {
                    kind: BodyKind2d::Dynamic,
                    position: [(i % 64) as f32 * 1.1 - 32., (i / 64) as f32 * 1.1 + 0.5],
                    ..Default::default()
                },
                Collider2d::default(),
            ));
        }
        let mut physics = Physics2d::new(60, [0., -9.81])?;
        let mut times = Vec::new();
        for i in 0..180 {
            let at = Instant::now();
            physics.step(&mut world, 1. / 60.)?;
            if i >= 60 {
                times.push(at.elapsed().as_secs_f64() * 1000.);
            }
        }
        times.sort_by(f64::total_cmp);
        let at = Instant::now();
        let snapshot = physics.snapshot()?;
        let snapshot_ms = at.elapsed().as_secs_f64() * 1000.;
        let at = Instant::now();
        physics.restore(&snapshot)?;
        let restore_ms = at.elapsed().as_secs_f64() * 1000.;
        report.push(serde_json::json!({"bodies":count+1,"step_p50_ms":times[times.len()/2],"step_p95_ms":times[times.len()*95/100],"snapshot_bytes":snapshot.len(),"snapshot_ms":snapshot_ms,"restore_ms":restore_ms,"samples":times.len()}));
    }
    let output = serde_json::json!({"backend":"Rapier2D enhanced-determinism, serial","profile":"dev opt-level=1, dependencies=3","hz":60,"includes":"stable-ID ECS sync, collider validation, solver step and component writeback","results":report});
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("measurements");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("solver.json"), ara_2d::canonical_json(&output))?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
