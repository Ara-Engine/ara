//! The Outpost sample with hot reload: a door swings, a survivor heals with medkits and
//! bodies fall, while the compile server watches `dotnet/samples/Outpost.Sample`. Edit
//! `Door.cs`, `Vitals.cs` or `Motion.cs` while it runs and watch the state change.
//!
//! ```sh
//! dotnet build dotnet/Ara.slnx -c Release
//! cargo run -p ara-script --example outpost -- [--seconds 60]
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_script::abi::Tick;
use ara_script::{EngineBindings, ModuleSpec, ScriptConfig, ScriptHost, bind_component, value};
use bevy_ecs::component::Component;
use bevy_ecs::world::World;
use serde_json::json;

/// The engine `Transform` (canonical layout of `ara_components::Transform`).
#[derive(Component, Clone, Copy, Default)]
#[repr(C)]
struct EcsTransform {
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

fn bin(project: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dotnet/artifacts/bin")
        .join(project)
        .join("release")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: u64 = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1)?.parse().ok())
        .unwrap_or(60);
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet/samples/Outpost.Sample");

    let db = ara_runtime::schema(&[]).expect("engine schema");
    let engine = bind_component!(EngineBindings::new(), EcsTransform as "Transform" { position, rotation, scale });
    let mut world = World::new();
    let mut config = ScriptConfig::jit(bin("Ara.Host"));
    config.compile_server = Some(bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let mut host = ScriptHost::start(config, db, &engine, &mut world)
        .unwrap_or_else(|e| panic!("start: {e:#?}"));
    let spec = ModuleSpec::from_dir(&sample).expect("sample module");
    host.load_modules(&mut world, &[spec])
        .unwrap_or_else(|e| panic!("load: {e:#?}"));
    println!(
        "watching {} for {seconds} s; edit its .cs files",
        sample.display()
    );

    let door = world.spawn(EcsTransform::default()).id();
    host.insert_component(
        &mut world,
        door,
        "Door",
        &json!({ "openAngle": 110, "isOpen": true }),
    )
    .expect("door");
    let survivor = world.spawn_empty().id();
    host.insert_component(
        &mut world,
        survivor,
        "Vitality",
        &json!({ "hp": 20, "medkits": 3 }),
    )
    .expect("vitality");
    let body = world.spawn(EcsTransform::default()).id();
    host.insert_component(&mut world, body, "SampleVelocity", &json!({}))
        .expect("velocity");
    host.insert_component(&mut world, body, "Mass", &json!({ "value": 80 }))
        .expect("mass");

    let dt = 1.0 / 60.0;
    let start = Instant::now();
    let mut next = start;
    let mut frame = 0u64;
    while start.elapsed() < Duration::from_secs(seconds) {
        let boundary = host.tick_boundary(&mut world);
        for r in boundary.reports {
            println!(
                "reload: kind {} escalation {} hot {} generation {}",
                r.kind, r.escalation, r.hot, r.generation
            );
        }
        for d in host.take_diagnostics() {
            println!("{d}");
        }
        host.run_tick(
            &mut world,
            Tick {
                frame,
                seed: 1,
                elapsed: frame as f64 * dt,
                dt: dt as f32,
                reserved: 0,
            },
        );
        if frame % 120 == 60 {
            let actor = value::entity_json(survivor.to_bits());
            match host.execute_command(
                &mut world,
                "game.UseMedkit",
                &json!({ "actor": actor }),
                Some(survivor),
            ) {
                Ok(record) => println!("UseMedkit: {}", host.describe(&record)["ops"][0]["after"]),
                Err(d) => println!("UseMedkit: {}", d.message),
            }
        }
        if frame.is_multiple_of(60) {
            let angle = world
                .get::<EcsTransform>(door)
                .map_or(0.0, |t| t.rotation[1]);
            let y = world
                .get::<EcsTransform>(body)
                .map_or(0.0, |t| t.position[1]);
            let vitality = host
                .read_component(&world, survivor, "Vitality")
                .unwrap_or_default();
            println!(
                "t={:>3}s door {angle:>6.1} deg, body y {y:>8.2} m, survivor {vitality}",
                frame / 60
            );
        }
        frame += 1;
        next += Duration::from_secs_f64(dt);
        if let Some(gap) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(gap);
        }
    }
    println!("leaked generations: {}", host.shutdown());
}
