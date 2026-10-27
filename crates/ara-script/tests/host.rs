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
    let mut host = ScriptHost::start(
        ScriptConfig::jit(support::bin("Ara.Host")),
        db,
        &engine,
        world,
    )
    .expect("host starts");
    let specs: Vec<ModuleSpec> = modules
        .iter()
        .map(|(project, dir)| {
            let mut spec = ModuleSpec::from_dir(&support::dotnet().join(dir)).expect("module file");
            spec.assembly = Some(support::bin(project).join(format!("{project}.dll")));
            spec.watch = false;
            spec
        })
        .collect();
    host.load_modules(world, &specs)
        .unwrap_or_else(|e| panic!("modules load: {e:#?}"));
    host
}

fn sample(world: &mut World) -> ScriptHost {
    host(world, &[("Outpost.Sample", "samples/Outpost.Sample")])
}

fn tick(frame: u64) -> Tick {
    Tick {
        frame,
        seed: 7,
        elapsed: frame as f64 * 0.1,
        dt: 0.1,
        reserved: 0,
    }
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
    assert_eq!(
        types["UseMedkitParams"]["fields"]["actor"]["required"],
        true
    );
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
    assert_eq!(
        (m.name.as_str(), m.hot, m.counts),
        ("Outpost.Sample", false, (2, 1, 1))
    );
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
        host.insert_component(&mut world, e, "Velocity", &json!({ "linear": [1, 0, 0] }))
            .expect("velocity");
        let mass = if i % 3 == 0 { 0.0 } else { 2.0 };
        host.insert_component(&mut world, e, "Mass", &json!({ "value": mass }))
            .expect("mass");
        if i % 2 == 0 {
            host.insert_component(&mut world, e, "Vitality", &json!({ "hp": 10 }))
                .expect("third table");
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
    assert!(
        host.diag(DIAG_THREADS_SEEN) > 1,
        "C# systems ran on task-pool threads"
    );
    assert!(errors(&host.take_diagnostics()).is_empty());
}

#[test]
fn behaviours_tick_with_ecs_backed_properties() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = sample(&mut world);
    let door = world.spawn(EcsTransform::default()).id();
    host.insert_component(
        &mut world,
        door,
        "Door",
        &json!({ "openAngle": 90, "isOpen": true }),
    )
    .expect("door");
    for f in 0..5 {
        host.run_tick(&mut world, tick(f));
    }
    // 90 deg/s for 0.5 s.
    assert_eq!(
        world.get::<EcsTransform>(door).expect("transform").rotation[1],
        45.0
    );
    host.insert_component(
        &mut world,
        door,
        "Door",
        &json!({ "openAngle": 90, "isOpen": false }),
    )
    .expect("close");
    host.run_tick(&mut world, tick(5));
    assert_eq!(
        world.get::<EcsTransform>(door).expect("transform").rotation[1],
        36.0
    );
    assert!(errors(&host.take_diagnostics()).is_empty());
}

/// Repro for process-global loader/callback ownership; the integrator routes its fix.
#[test]
#[ignore = "multi-world C# host lifecycle is routed to the engine owner"]
fn two_worlds_drop_one_host_and_the_other_keeps_ticking() {
    let _clr = support::clr_lock();
    let mut first_world = World::new();
    let first = sample(&mut first_world);
    let mut second_world = World::new();
    let mut second = sample(&mut second_world);
    let door = second_world.spawn(EcsTransform::default()).id();
    second
        .insert_component(
            &mut second_world,
            door,
            "Door",
            &json!({"openAngle":90,"isOpen":true}),
        )
        .expect("door");
    second.run_tick(&mut second_world, tick(1));
    assert_eq!(
        second_world
            .get::<EcsTransform>(door)
            .expect("transform")
            .rotation[1],
        9.
    );
    drop(first);
    drop(first_world);
    second.run_tick(&mut second_world, tick(2));
    assert_eq!(
        second_world
            .get::<EcsTransform>(door)
            .expect("transform")
            .rotation[1],
        18.,
        "dropping one world must preserve another world's module dispatch"
    );
    assert!(errors(&second.take_diagnostics()).is_empty());
}

fn survivor(host: &ScriptHost, world: &mut World, hp: u16, medkits: u8) -> Entity {
    let e = world.spawn_empty().id();
    host.insert_component(
        world,
        e,
        "Vitality",
        &json!({ "hp": hp, "medkits": medkits }),
    )
    .expect("vitality");
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
        .execute_command(
            &mut world,
            "game.UseMedkit",
            &json!({ "actor": actor(e) }),
            Some(e),
        )
        .expect("heals");
    assert_eq!(
        host.read_component(&world, e, "Vitality"),
        Some(json!({ "hp": 75, "medkits": 1 }))
    );
    let described = host.describe(&record);
    assert_eq!(
        described["ops"][0]["before"],
        json!({ "hp": 50, "medkits": 2 })
    );
    assert_eq!(
        described["cues"],
        json!([{ "cue": "MedkitUsed", "value": { "actor": actor(e) } }])
    );
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(
        host.read_component(&world, e, "Vitality"),
        Some(json!({ "hp": 50, "medkits": 2 }))
    );

    // A rejection changes nothing.
    let full = survivor(&host, &mut world, 100, 3);
    let err = host
        .execute_command(
            &mut world,
            "game.UseMedkit",
            &json!({ "actor": actor(full) }),
            None,
        )
        .expect_err("full health");
    assert_eq!(err.code, codes::COMMAND_REJECTED);
    assert!(err.message.contains("cannot_heal"), "{}", err.message);
    assert_eq!(
        host.read_component(&world, full, "Vitality"),
        Some(json!({ "hp": 100, "medkits": 3 }))
    );

    // Params are validated against the C#-declared params type.
    let err = host
        .execute_command(&mut world, "game.UseMedkit", &json!({}), None)
        .expect_err("missing actor");
    assert_eq!(
        (err.code, err.location.path.as_deref()),
        (codes::INVALID_PARAMS, Some("params.actor"))
    );
    let err = host
        .execute_command(&mut world, "game.UseMedkt", &json!({}), None)
        .expect_err("typo");
    assert!(
        err.message.contains("did you mean `game.UseMedkit`"),
        "{}",
        err.message
    );
}

#[test]
fn faults_are_contained_and_structural_commands_undo() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = host(&mut world, &[("Ara.TestModule", "tests/Ara.TestModule")]);
    let calm: Vec<Entity> = (0..100)
        .map(|_| {
            let e = world.spawn_empty().id();
            host.insert_component(&mut world, e, "Counter", &json!({}))
                .expect("counter");
            e
        })
        .collect();
    let lit = world.spawn_empty().id();
    for name in ["Counter", "Bomb"] {
        host.insert_component(&mut world, lit, name, &json!({}))
            .expect("component");
    }
    host.insert_component(&mut world, lit, "Fuse", &json!({ "lit": true }))
        .expect("fuse");
    for f in 0..3 {
        host.run_tick(&mut world, tick(f));
    }
    // `Explode` and `Bomb` threw; each is disabled, reported once, and `Count` kept running.
    let diags = host.take_diagnostics();
    let faults: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.code == codes::SYSTEM_FAULT || d.code == codes::BEHAVIOUR_FAULT)
        .collect();
    assert_eq!(faults.len(), 2, "{diags:#?}");
    assert!(
        faults
            .iter()
            .any(|d| d.message.contains("Counting.Explode") && d.message.contains("fuse lit"))
    );
    let at = faults
        .iter()
        .find(|d| d.code == codes::SYSTEM_FAULT)
        .expect("system fault");
    assert!(
        at.location
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("Probes.cs"))
            && at.location.line.is_some(),
        "{at:?}"
    );
    for e in calm.iter().chain([&lit]) {
        assert_eq!(
            host.read_component(&world, *e, "Counter").expect("counter")["value"],
            3
        );
    }
    let sum = host
        .read_component(&world, calm[7], "Counter")
        .expect("counter")["entitySum"]
        .as_u64();
    assert_eq!(sum, Some(3 * u64::from(calm[7].index().index())));

    // A throwing command rolls back its writes.
    let err = host
        .execute_command(
            &mut world,
            "test.Fail",
            &json!({ "target": actor(calm[0]) }),
            None,
        )
        .expect_err("throws");
    assert_eq!(err.code, codes::COMMAND_FAULT);
    assert!(
        err.message.contains("command failed on purpose"),
        "{}",
        err.message
    );
    assert_eq!(
        host.read_component(&world, calm[0], "Counter")
            .expect("counter")["value"],
        3
    );
    let err = host
        .execute_command(
            &mut world,
            "test.Fail",
            &json!({ "target": actor(calm[0]) }),
            None,
        )
        .expect_err("disabled");
    assert!(err.message.contains("disabled"), "{}", err.message);

    // Structural ops apply after the command and undo in reverse.
    let before = world.entities().count_spawned();
    let record = host
        .execute_command(
            &mut world,
            "test.Shuffle",
            &json!({ "victim": actor(calm[1]), "count": 4 }),
            None,
        )
        .expect("shuffles");
    assert_eq!(world.entities().count_spawned(), before + 3);
    assert!(world.get_entity(calm[1]).is_err());
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(world.entities().count_spawned(), before);
    assert_eq!(host.shutdown(), 0, "every module generation unloaded");
}

/// Every entity whose `Counter` value is `value`.
fn counters(host: &ScriptHost, world: &mut World, value: i64) -> Vec<Entity> {
    let all: Vec<Entity> = world.query::<Entity>().iter(world).collect();
    all.into_iter()
        .filter(|e| {
            host.read_component(world, *e, "Counter")
                .is_some_and(|c| c["value"] == value)
        })
        .collect()
}

#[test]
fn commands_journal_resources_and_undo_all_or_nothing() {
    let _clr = support::clr_lock();
    let mut world = World::new();
    let mut host = host(&mut world, &[("Ara.TestModule", "tests/Ara.TestModule")]);
    let tally = |host: &mut ScriptHost, world: &mut World, reject: bool| {
        host.execute_command(
            world,
            "test.Tally",
            &json!({ "add": 5, "reject": reject }),
            None,
        )
    };

    // A rejected call restores the resource it wrote; an applied one undoes.
    let err = tally(&mut host, &mut world, true).expect_err("rejects");
    assert_eq!(err.code, codes::COMMAND_REJECTED);
    let record = tally(&mut host, &mut world, false).expect("adds");
    let set = json!({ "op": "set_resource", "resource": "Tally", "before": { "total": 0 }, "after": { "total": 5 } });
    assert_eq!(host.describe(&record)["ops"], json!([set]));
    host.undo(&mut world, &record).expect("undo");
    let again = tally(&mut host, &mut world, false).expect("adds");
    assert_eq!(host.describe(&again)["ops"], json!([set]));

    // Undo respawns the despawned victim and restores the write made before the despawn.
    let victim = world.spawn_empty().id();
    host.insert_component(&mut world, victim, "Counter", &json!({ "value": 9 }))
        .expect("counter");
    let before = world.entities().count_spawned();
    let record = host
        .execute_command(
            &mut world,
            "test.Shuffle",
            &json!({ "victim": actor(victim), "count": 2 }),
            None,
        )
        .expect("shuffles");
    let spawned: Vec<Entity> = record
        .structural
        .iter()
        .filter_map(|a| match a {
            ara_script::Applied::Spawn { entity } => Entity::try_from_bits(*entity),
            _ => None,
        })
        .collect();
    assert_eq!(spawned.len(), 2);
    for e in &spawned {
        assert_eq!(
            host.read_component(&world, *e, "Frozen"),
            Some(json!({})),
            "tag inserted"
        );
    }
    host.undo(&mut world, &record).expect("undo");
    assert_eq!(world.entities().count_spawned(), before);
    let restored = counters(&host, &mut world, 9);
    assert_eq!(
        restored.len(),
        1,
        "the victim came back with its old counter"
    );

    // An undo that cannot finish changes nothing.
    let record = host
        .execute_command(
            &mut world,
            "test.Shuffle",
            &json!({ "victim": actor(restored[0]), "count": 2 }),
            None,
        )
        .expect("shuffles");
    let gone = record
        .structural
        .iter()
        .find_map(|a| match a {
            ara_script::Applied::Spawn { entity } => Entity::try_from_bits(*entity),
            _ => None,
        })
        .expect("a spawn");
    world.despawn(gone);
    let now = world.entities().count_spawned();
    let err = host
        .undo(&mut world, &record)
        .expect_err("a spawned entity is gone");
    assert_eq!(err.code, codes::UNDO_FAILED);
    assert_eq!(world.entities().count_spawned(), now);
    assert!(
        counters(&host, &mut world, 9).is_empty(),
        "the victim was not respawned"
    );
    assert_eq!(host.shutdown(), 0, "every module generation unloaded");
}
