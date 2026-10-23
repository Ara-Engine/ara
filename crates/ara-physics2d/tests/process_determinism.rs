//! Cross-process determinism: two fresh processes, opposite ECS insertion orders,
//! must produce byte-identical solver checkpoints after 300 ticks.
use anyhow::{Result, ensure};
use ara_physics2d::*;
use ara_sim::{Identity, bevy_ecs::prelude::World};

fn simulate(reverse: bool) -> Result<Vec<u8>> {
    let mut world = World::new();
    let mut bodies: Vec<_> = (0..64)
        .map(|i| {
            (
                Identity(format!("e_body{i:04}")),
                RigidBody2d {
                    motion: MotionType2d::Dynamic,
                    ..Default::default()
                },
                Collider2d {
                    kind: if i % 3 == 0 {
                        ColliderKind2d::Circle
                    } else {
                        ColliderKind2d::Box
                    },
                    ..Default::default()
                },
                PhysicsPose2d {
                    position: [(i % 16) as f32 * 1.1, (i / 16) as f32 * 1.1 + 1.],
                    rotation: i as f32 * 0.1,
                },
            )
        })
        .collect();
    if reverse {
        bodies.reverse();
    }
    for body in bodies {
        world.spawn(body);
    }
    world.spawn((
        Identity("e_floor".into()),
        RigidBody2d::default(),
        Collider2d {
            half_extents: [30., 0.5],
            ..Default::default()
        },
        PhysicsPose2d {
            position: [0., -0.5],
            rotation: 0.,
        },
    ));
    world.spawn((
        Identity("e_hero".into()),
        CharacterController2d::default(),
        CharacterInput2d {
            desired_velocity: [2., 0.],
            jump_speed: 0.,
        },
        PhysicsPose2d {
            position: [-6., 1.],
            rotation: 0.,
        },
    ));
    world.insert_resource(Physics2d::new(Physics2dConfig::default())?);
    for _ in 0..300 {
        tick_ecs(&mut world)?;
    }
    world.resource::<Physics2d>().snapshot()
}

fn main() -> Result<()> {
    if let Some(path) = std::env::var_os("ARA_2D_REPLAY_OUTPUT") {
        let reverse = std::env::var_os("ARA_2D_REVERSE").is_some();
        std::fs::write(path, simulate(reverse)?)?;
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("forward.bin");
    let b = dir.path().join("reverse.bin");
    for (path, reverse) in [(&a, false), (&b, true)] {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .env("ARA_2D_REPLAY_OUTPUT", path)
            .env_remove("ARA_2D_REVERSE");
        if reverse {
            command.env("ARA_2D_REVERSE", "1");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: keep worker processes from flashing consoles.
            command.creation_flags(0x08000000);
        }
        ensure!(command.status()?.success(), "determinism worker failed");
    }
    ensure!(
        std::fs::read(a)? == std::fs::read(b)?,
        "cross-process physics snapshots diverged"
    );
    println!(
        "Cross-process determinism PASS: 66 bodies + character, 300 ticks, reversed ECS order"
    );
    Ok(())
}
