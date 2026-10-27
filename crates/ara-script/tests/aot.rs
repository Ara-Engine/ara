//! The sample module as a NativeAOT image (the shipping shape) against the same host:
//! registration, systems, behaviours and commands behave as under the JIT.

mod support;

use ara_script::abi::{DIAG_CAN_RELOAD, Tick};
use ara_script::{ScriptConfig, ScriptHost, codes};
use bevy_ecs::world::World;
use serde_json::json;
use support::EcsTransform;

#[test]
fn native_aot_worlds_share_the_image_but_keep_dispatch_and_shutdown_separate() {
    let _clr = support::clr_lock();
    let library = support::publish_aot();
    let (db, engine) = support::engine();
    let mut first_world = World::new();
    let mut first = ScriptHost::start(
        ScriptConfig::native_aot(&library),
        db.clone(),
        &engine,
        &mut first_world,
    )
    .expect("first host");
    first
        .load_modules(&mut first_world, &[])
        .expect("first modules");
    let mut second_world = World::new();
    let mut second = ScriptHost::start(
        ScriptConfig::native_aot(&library),
        db,
        &engine,
        &mut second_world,
    )
    .expect("second host");
    second
        .load_modules(&mut second_world, &[])
        .expect("second modules");
    let a = first_world.spawn(EcsTransform::default()).id();
    let b = second_world.spawn(EcsTransform::default()).id();
    for (host, world, door, angle) in [
        (&mut first, &mut first_world, a, 3),
        (&mut second, &mut second_world, b, 90),
    ] {
        host.insert_component(
            world,
            door,
            "Door",
            &json!({"openAngle":angle,"isOpen":true}),
        )
        .expect("door");
        host.run_tick(
            world,
            Tick {
                frame: 0,
                seed: 1,
                elapsed: 0.0,
                dt: 0.1,
                reserved: 0,
            },
        );
    }
    assert_eq!(
        first_world
            .get::<EcsTransform>(a)
            .expect("first transform")
            .rotation[1],
        3.0
    );
    assert_eq!(
        second_world
            .get::<EcsTransform>(b)
            .expect("second transform")
            .rotation[1],
        9.0
    );
    assert_eq!(first.shutdown(), 0);
    second.run_tick(
        &mut second_world,
        Tick {
            frame: 1,
            seed: 1,
            elapsed: 0.1,
            dt: 0.1,
            reserved: 0,
        },
    );
    assert_eq!(
        second_world
            .get::<EcsTransform>(b)
            .expect("survivor transform")
            .rotation[1],
        18.0
    );
    assert_eq!(second.shutdown(), 0);
}

#[test]
fn native_aot_sample_runs_against_the_host() {
    let _clr = support::clr_lock();
    let library = support::publish_aot();
    let mut world = World::new();
    let (db, engine) = support::engine();
    let mut host = ScriptHost::start(ScriptConfig::native_aot(&library), db, &engine, &mut world)
        .expect("AOT image starts");
    host.load_modules(&mut world, &[])
        .unwrap_or_else(|e| panic!("linked modules register: {e:#?}"));
    assert_eq!(
        host.diag(DIAG_CAN_RELOAD),
        0,
        "NativeAOT images do not reload"
    );
    assert_eq!(host.modules()[0].name, "Outpost.Sample");
    assert_eq!(
        host.schema().get("Vitality").map(|t| t.module.as_str()),
        Some("Outpost.Sample")
    );

    // Gravity + Movement (systems), Door (behaviour).
    let body = world.spawn(EcsTransform::default()).id();
    host.insert_component(&mut world, body, "SampleVelocity", &json!({}))
        .expect("velocity");
    host.insert_component(&mut world, body, "Mass", &json!({ "value": 1 }))
        .expect("mass");
    let door = world.spawn(EcsTransform::default()).id();
    host.insert_component(
        &mut world,
        door,
        "Door",
        &json!({ "openAngle": 90, "isOpen": true }),
    )
    .expect("door");
    for frame in 0..5 {
        host.run_tick(
            &mut world,
            Tick {
                frame,
                seed: 3,
                elapsed: frame as f64 * 0.1,
                dt: 0.1,
                reserved: 0,
            },
        );
    }
    let v = host
        .read_component(&world, body, "SampleVelocity")
        .expect("velocity");
    assert!(
        (v["linear"][1].as_f64().unwrap_or(0.0) + 4.905).abs() < 1e-4,
        "{v}"
    );
    assert_eq!(
        world.get::<EcsTransform>(door).expect("transform").rotation[1],
        45.0
    );

    // UseMedkit (command) and its undo.
    let survivor = world.spawn_empty().id();
    host.insert_component(
        &mut world,
        survivor,
        "Vitality",
        &json!({ "hp": 40, "medkits": 1 }),
    )
    .expect("vitality");
    let actor = ara_script::value::entity_json(survivor.to_bits());
    let record = host
        .execute_command(
            &mut world,
            "game.UseMedkit",
            &json!({ "actor": actor }),
            None,
        )
        .expect("heals");
    assert_eq!(
        host.read_component(&world, survivor, "Vitality"),
        Some(json!({ "hp": 65, "medkits": 0 }))
    );
    let again = host
        .execute_command(
            &mut world,
            "game.UseMedkit",
            &json!({ "actor": actor }),
            None,
        )
        .expect_err("no medkits");
    assert_eq!(again.code, codes::COMMAND_REJECTED);
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(
        host.read_component(&world, survivor, "Vitality"),
        Some(json!({ "hp": 40, "medkits": 1 }))
    );
    assert!(host.take_diagnostics().iter().all(|d| !d.is_error()));
}
