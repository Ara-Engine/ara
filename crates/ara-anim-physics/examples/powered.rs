//! Real-Jolt powered-ragdoll crowd: drives, fixed physics and post-physics pose blending.
#[path = "../../ara-anim/examples/v2_support/mod.rs"]
mod support;
use ara_anim::{physical::*, *};
use ara_anim_physics::JoltRagdoll;
use ara_physics::PhysicsConfig;
use ara_physics_jolt::JoltWorld;
use std::{hint::black_box, time::Instant};
struct Actor {
    rig: JoltRagdoll,
    physical: PhysicalAnimation,
    state: PhysicalState,
    pose: Pose,
    root: RootMotion,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let s = support::skeleton(1.0, 20)?;
    let profile = RagdollProfile {
        bones: [
            ("Hips", None),
            ("Chest", Some(0)),
            ("Head", Some(1)),
            ("LeftUpperLeg", Some(0)),
            ("LeftLowerLeg", Some(3)),
            ("RightUpperLeg", Some(0)),
            ("RightLowerLeg", Some(5)),
        ]
        .into_iter()
        .map(|(joint, parent)| RagdollBone {
            joint: joint.into(),
            parent,
            ..Default::default()
        })
        .collect(),
    };
    let mut config = PhysicsConfig {
        threads: 8,
        max_bodies: 1024,
        event_capacity: 0,
        ..Default::default()
    };
    config.layer_masks[2] &= !(1 << 2);
    let mut world = JoltWorld::new(config)?;
    let mut actors = Vec::new();
    for i in 0..32 {
        let root = RootMotion {
            translation: glam::Vec3::new((i % 8) as f32 * 3.0, 0.0, (i / 8) as f32 * 3.0),
            ..Default::default()
        };
        let mut state = PhysicalState::default();
        state.request(PhysicalMode::HitReaction, 1, 0.5)?;
        state.tick()?;
        actors.push(Actor {
            rig: JoltRagdoll::spawn(&mut world, &s, &profile, root, 10 + i * 8, 2)?,
            physical: PhysicalAnimation::cook(&s, &profile)?,
            state,
            pose: Pose::new(s.len()),
            root,
        });
    }
    let mut samples = Vec::new();
    for frame in 0..180 {
        let start = Instant::now();
        for actor in &mut actors {
            actor.physical.drive(
                &s,
                s.bind_pose(),
                actor.root,
                &actor.state,
                &mut actor.rig.bind(&mut world),
            )?;
        }
        world.step()?;
        for actor in &mut actors {
            actor.physical.blend(
                &s,
                s.bind_pose(),
                actor.root,
                &actor.state,
                &actor.rig.bind(&mut world),
                &mut actor.pose,
            )?;
            black_box(&actor.pose);
        }
        if frame >= 20 {
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "32x7_bodies_8workers_drive_step_blend_ms p50={:.3} p95={:.3}",
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100]
    );
    Ok(())
}
