use anyhow::{Result, ensure};
use ara_physics2d::{Body2d, BodyKind2d, Collider2d, Physics2d};
use ara_sim::{Identity, bevy_ecs::prelude::World};
fn main() -> Result<()> {
    if let Some(path) = std::env::var_os("ARA_2D_REPLAY_OUTPUT") {
        let mut world = World::new();
        let mut inputs = Vec::new();
        for i in 0..64 {
            inputs.push((
                Identity(format!("e_body{i:04}")),
                Body2d {
                    kind: BodyKind2d::Dynamic,
                    position: [(i % 16) as f32 * 1.1, (i / 16) as f32 * 1.1 + 1.],
                    ..Default::default()
                },
                Collider2d::default(),
            ));
        }
        if std::env::var_os("ARA_2D_REVERSE").is_some() {
            inputs.reverse();
        }
        for input in inputs {
            world.spawn(input);
        }
        world.spawn((
            Identity("e_floor".into()),
            Body2d {
                position: [0., -0.5],
                ..Default::default()
            },
            Collider2d {
                half_extents: [30., 0.5],
                ..Default::default()
            },
        ));
        let mut physics = Physics2d::new(60, [0., -9.81])?;
        for _ in 0..300 {
            physics.step(&mut world, 1. / 60.)?;
        }
        std::fs::write(path, physics.snapshot()?)?;
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
            command.creation_flags(0x08000000);
        }
        ensure!(command.status()?.success(), "determinism worker failed");
    }
    ensure!(
        std::fs::read(a)? == std::fs::read(b)?,
        "cross-process physics snapshots diverged"
    );
    println!("Cross-process determinism PASS: 65 bodies, 300 ticks, reversed ECS insertion");
    Ok(())
}
