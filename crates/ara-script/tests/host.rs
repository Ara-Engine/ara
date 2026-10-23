//! The C# host against prebuilt modules (no compile server): registration round trip,
//! systems over chunks, behaviours, commands with undo, and fault containment.

mod support;

use ara_diag::Diagnostic;
use ara_script::abi::{DIAG_THREADS_SEEN, Tick};
use ara_script::{ModuleSpec, ScriptConfig, ScriptHost, codes};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};
use support::EcsTransform;

/// Starts a host with the given prebuilt modules loaded.
fn host(world: &mut World, modules: &[(&str, &str)]) -> ScriptHost {
    support::build();
    let (db, engine) = support::engine();
    let mut host = ScriptHost::start(ScriptConfig::jit(support::bin("Ara.Host")), db, &engine, world).expect("host starts");
    let specs: Vec<ModuleSpec> = modules
        .iter()
        .map(|(project, dir)| {
            let mut spec = ModuleSpec::from_dir(&support::dotnet().join(dir)).expect("module file");
            spec.assembly = Some(support::bin(project).join(format!("{project}.dll")));
            spec.watch = false;
            spec
        })
        .collect();
    host.load_modules(world, &specs).unwrap_or_else(|e| panic!("modules load: {e:#?}"));
    host
}

fn sample(world: &mut World) -> ScriptHost {
    host(world, &[("Outpost.Sample", "samples/Outpost.Sample")])
}

fn tick(frame: u64) -> Tick {
    Tick { frame, seed: 7, elapsed: frame as f64 * 0.1, dt: 0.1, reserved: 0 }
}

fn errors(d: &[Diagnostic]) -> Vec<&Diagnostic> {
    d.iter().filter(|d| d.is_error()).collect()
}

#[test]
fn csharp_types_register_into_the_schema_db() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let host = sample(&mut world);
    let db = host.schema();
    let types = &db.reflection()["types"];
    assert_eq!(
        types["Vitality"],
        json!({
            "kind": "component",
            "doc": "Health and healing supplies of a survivor.",
            "module": "Outpost.Sample",
            "fields": {
                "hp": { "type": "u16", "doc": "Hit points; 0 is downed.", "default": 0, "range": [0.0, 100.0], "replicate": { "audience": "relevant" } },
                "medkits": { "type": "u8", "doc": "Medkits carried.", "default": 0, "range": [0.0, 9.0], "replicate": { "audience": "owner" } }
            }
        })
    );
    assert_eq!(types["Door"]["kind"], "component");
    assert_eq!(types["Door"]["fields"]["openAngle"]["unit"], "deg");
    assert_eq!(types["GameRules"]["kind"], "resource");
    assert_eq!(types["UseMedkitParams"]["fields"]["actor"]["required"], true);
    assert_eq!(types["MedkitUsed"]["kind"], "struct");
    // The JSON Schema generator works on C# types like on Rust ones.
    let schema = db.type_json_schema(db.get("Vitality").expect("registered"));
    assert_eq!(schema["properties"]["hp"]["maximum"], 100.0);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    let catalog = host.command_catalog();
    assert_eq!(catalog["game.UseMedkit"]["params"], "UseMedkitParams");
    assert_eq!(catalog["game.UseMedkit"]["writes"], json!(["Vitality"]));
    assert_eq!(catalog["game.UseMedkit"]["expose"]["caller"], "owner");
    let m = &host.modules()[0];
    assert_eq!((m.name.as_str(), m.hot, m.counts), ("Outpost.Sample", false, (2, 1, 1)));
}

/// Gravity then Movement (ordered: both touch Velocity) over three tables, checked
/// against the same f32 arithmetic in Rust.
#[test]
fn systems_run_over_every_chunk_in_parallel() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = sample(&mut world);
    let mut bodies = Vec::new();
    for i in 0..30_000u32 {
        let e = world.spawn(EcsTransform::default()).id();
        host.insert_component(&mut world, e, "Velocity", &json!({ "linear": [1, 0, 0] })).expect("velocity");
        let mass = if i % 3 == 0 { 0.0 } else { 2.0 };
        host.insert_component(&mut world, e, "Mass", &json!({ "value": mass })).expect("mass");
        if i % 2 == 0 {
            host.insert_component(&mut world, e, "Vitality", &json!({ "hp": 10 })).expect("third table");
        }
        bodies.push((e, mass));
    }
    for f in 0..10 {
        host.tick_boundary(&mut world);
        host.run_tick(&mut world, tick(f));
    }
    let (g, dt) = (9.81f32, 0.1f32);
    for &(e, mass) in bodies.iter().step_by(997) {
        let (mut vy, mut py, mut px) = (0f32, 0f32, 0f32);
        for _ in 0..10 {
            if mass > 0.0 {
                vy -= g * dt;
            }
            py += vy * dt;
            px += 1.0 * dt;
        }
        let t = world.get::<EcsTransform>(e).expect("transform");
        assert_eq!(t.position, [px, py, 0.0], "entity {e} (mass {mass})");
    }
    assert!(host.diag(DIAG_THREADS_SEEN) > 1, "C# systems ran on task-pool threads");
    assert!(errors(&host.take_diagnostics()).is_empty());
}

#[test]
fn behaviours_tick_with_ecs_backed_properties() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = sample(&mut world);
    let door = world.spawn(EcsTransform::default()).id();
    host.insert_component(&mut world, door, "Door", &json!({ "openAngle": 90, "isOpen": true })).expect("door");
    for f in 0..5 {
        host.run_tick(&mut world, tick(f));
    }
    // 90 deg/s for 0.5 s.
    assert_eq!(world.get::<EcsTransform>(door).expect("transform").rotation[1], 45.0);
    host.insert_component(&mut world, door, "Door", &json!({ "openAngle": 90, "isOpen": false })).expect("close");
    host.run_tick(&mut world, tick(5));
    assert_eq!(world.get::<EcsTransform>(door).expect("transform").rotation[1], 36.0);
    assert!(errors(&host.take_diagnostics()).is_empty());
}

fn survivor(host: &ScriptHost, world: &mut World, hp: u16, medkits: u8) -> Entity {
    let e = world.spawn_empty().id();
    host.insert_component(world, e, "Vitality", &json!({ "hp": hp, "medkits": medkits })).expect("vitality");
    e
}

fn actor(e: Entity) -> Value {
    ara_script::value::entity_json(e.to_bits())
}

#[test]
fn commands_apply_as_one_undoable_step() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = sample(&mut world);
    let e = survivor(&host, &mut world, 50, 2);
    let record = host
        .execute_command(&mut world, "game.UseMedkit", &json!({ "actor": actor(e) }), Some(e))
        .expect("heals");
    assert_eq!(host.read_component(&world, e, "Vitality"), Some(json!({ "hp": 75, "medkits": 1 })));
    let described = host.describe(&record);
    assert_eq!(described["ops"][0]["before"], json!({ "hp": 50, "medkits": 2 }));
    assert_eq!(described["cues"], json!([{ "cue": "MedkitUsed", "value": { "actor": actor(e) } }]));
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(host.read_component(&world, e, "Vitality"), Some(json!({ "hp": 50, "medkits": 2 })));

    // A rejection changes nothing.
    let full = survivor(&host, &mut world, 100, 3);
    let err = host
        .execute_command(&mut world, "game.UseMedkit", &json!({ "actor": actor(full) }), None)
        .expect_err("full health");
    assert_eq!(err.code, codes::COMMAND_REJECTED);
    assert!(err.message.contains("cannot_heal"), "{}", err.message);
    assert_eq!(host.read_component(&world, full, "Vitality"), Some(json!({ "hp": 100, "medkits": 3 })));

    // Params are validated against the C#-declared params type.
    let err = host.execute_command(&mut world, "game.UseMedkit", &json!({}), None).expect_err("missing actor");
    assert_eq!((err.code, err.location.path.as_deref()), (codes::INVALID_PARAMS, Some("params.actor")));
    let err = host.execute_command(&mut world, "game.UseMedkt", &json!({}), None).expect_err("typo");
    assert!(err.message.contains("did you mean `game.UseMedkit`"), "{}", err.message);
}

#[test]
fn faults_are_contained_and_structural_commands_undo() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = host(&mut world, &[("Ara.TestModule", "tests/Ara.TestModule")]);
    let calm: Vec<Entity> = (0..100)
        .map(|_| {
            let e = world.spawn_empty().id();
            host.insert_component(&mut world, e, "Counter", &json!({})).expect("counter");
            e
        })
        .collect();
    let lit = world.spawn_empty().id();
    for name in ["Counter", "Bomb"] {
        host.insert_component(&mut world, lit, name, &json!({})).expect("component");
    }
    host.insert_component(&mut world, lit, "Fuse", &json!({ "lit": true })).expect("fuse");
    for f in 0..3 {
        host.run_tick(&mut world, tick(f));
    }
    // `Explode` and `Bomb` threw; each is disabled, reported once, and `Count` kept running.
    let diags = host.take_diagnostics();
    let faults: Vec<&Diagnostic> = diags.iter().filter(|d| d.code == codes::SYSTEM_FAULT || d.code == codes::BEHAVIOUR_FAULT).collect();
    assert_eq!(faults.len(), 2, "{diags:#?}");
    assert!(faults.iter().any(|d| d.message.contains("Counting.Explode") && d.message.contains("fuse lit")));
    let at = faults.iter().find(|d| d.code == codes::SYSTEM_FAULT).expect("system fault");
    assert!(at.location.file.as_deref().is_some_and(|f| f.ends_with("Probes.cs")) && at.location.line.is_some(), "{at:?}");
    for e in calm.iter().chain([&lit]) {
        assert_eq!(host.read_component(&world, *e, "Counter").expect("counter")["value"], 3);
    }
    let sum = host.read_component(&world, calm[7], "Counter").expect("counter")["entitySum"].as_u64();
    assert_eq!(sum, Some(3 * u64::from(calm[7].index().index())));

    // A throwing command rolls back its writes.
    let err = host.execute_command(&mut world, "test.Fail", &json!({ "target": actor(calm[0]) }), None).expect_err("throws");
    assert_eq!(err.code, codes::COMMAND_FAULT);
    assert_eq!(host.read_component(&world, calm[0], "Counter").expect("counter")["value"], 3);

    // Structural ops apply after the command and undo in reverse.
    let before = world.entities().count_spawned();
    let record = host
        .execute_command(&mut world, "test.Shuffle", &json!({ "victim": actor(calm[1]), "count": 4 }), None)
        .expect("shuffles");
    assert_eq!(world.entities().count_spawned(), before + 3);
    assert!(world.get_entity(calm[1]).is_err());
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(world.entities().count_spawned(), before);
    assert_eq!(host.shutdown(), 0, "every module generation unloaded");
}
