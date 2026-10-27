//! Runs the compiled documentation recipes against the actual simulation codecs.
mod support;
use ara_script::{ModuleSpec, ScriptConfig, ScriptHost, abi::RELOAD_L3};
use bevy_ecs::world::World;
use serde_json::{Value, json};

#[test]
fn compiled_authoring_examples_match_the_real_command_parameter_schemas() {
    let _clr = support::clr_lock();
    support::build();
    let output = std::process::Command::new("dotnet")
        .arg(support::bin("Ara.CommandExamples").join("Ara.CommandExamples.dll"))
        .arg("--trace-commands")
        .output()
        .expect("compiled C# command examples");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls: Value = serde_json::from_slice(&output.stdout).expect("command trace");
    let mut db = ara_runtime::schema(&[]).expect("engine schema");
    let mut registry = ara_cmd::Registry::builtin(&mut db);
    ara_runtime::engine::register_commands(&mut registry, &mut db);
    for call in calls.as_array().expect("calls") {
        let name = call["command"].as_str().expect("command name");
        let info = registry
            .infos()
            .find(|info| info.name == name)
            .unwrap_or_else(|| panic!("missing command {name}"));
        let mut problems = Vec::new();
        ara_schema::Validator::new(&db, &mut problems).value(
            &ara_schema::ValueType::Named(info.params.clone()),
            None,
            &call["parameters"],
            name,
        );
        assert!(
            !problems.iter().any(ara_diag::Diagnostic::is_error),
            "stale C# example {name}: {problems:?}"
        );
    }
}

#[test]
fn compiled_csharp_examples_write_every_engine_component_and_undo_them() {
    let _clr = support::clr_lock();
    support::build();
    let (db, bindings) = support::engine();
    let mut world = World::new();
    let mut host = ScriptHost::start(
        ScriptConfig::jit(support::bin("Ara.Host")),
        db,
        &bindings,
        &mut world,
    )
    .expect("host");
    let mut module = ModuleSpec::from_dir(&support::dotnet().join("examples/Ara.Examples"))
        .expect("examples module");
    module.assembly = Some(support::bin("Ara.Examples").join("Ara.Examples.dll"));
    host.load_modules(&mut world, &[module])
        .expect("compiled examples load");
    host.run_tick(
        &mut world,
        ara_script::abi::Tick {
            frame: 1,
            seed: 1,
            elapsed: 1.0 / 60.0,
            dt: 1.0 / 60.0,
            reserved: 0,
        },
    );
    let entity = world.spawn_empty().id();
    let names: Vec<_> = host.schema().components().map(|t| t.name.clone()).collect();
    let mut before = Vec::new();
    for name in &names {
        let mut initial = json!({});
        // A required authoring entity reference is a stable record ID, not a CLR object.
        if name == "TileColliderSource2d" {
            initial["owner"] = json!("e_actor");
        }
        host.insert_component(&mut world, entity, name, &initial)
            .unwrap_or_else(|d| panic!("seed {name}: {d:?}"));
        before.push(
            host.read_component(&world, entity, name)
                .expect("seeded component"),
        );
    }
    let record = host
        .execute_command(
            &mut world,
            "examples.components",
            &json!({"target":entity.to_bits()}),
            None,
        )
        .expect("C# recipes execute");
    assert_eq!(
        record.writes.len(),
        names.len(),
        "each recipe changes at least one field"
    );
    for (name, was) in names.iter().zip(&before) {
        assert_ne!(
            host.read_component(&world, entity, name),
            Some(was.clone()),
            "{name}"
        );
    }
    host.undo(&mut world, &record)
        .expect("undo includes variable-sized values");
    for (name, was) in names.iter().zip(before) {
        assert_eq!(
            host.read_component(&world, entity, name),
            Some(was),
            "undo {name}"
        );
    }
    let audio_before = host.read_component(&world, entity, "AudioSource");
    let transform_before = host.read_component(&world, entity, "Transform");
    assert!(
        host.execute_command(
            &mut world,
            "examples.reject",
            &json!({"target":entity.to_bits()}),
            None
        )
        .is_err()
    );
    assert_eq!(
        host.read_component(&world, entity, "AudioSource"),
        audio_before
    );
    assert_eq!(
        host.read_component(&world, entity, "Transform"),
        transform_before
    );
    let light_before = host.read_component(&world, entity, "Light");
    let fault = host
        .execute_command(
            &mut world,
            "examples.fault",
            &json!({"target":entity.to_bits()}),
            None,
        )
        .expect_err("faulted callback discards its invalid copy");
    assert_eq!(fault.code, ara_script::codes::COMMAND_FAULT);
    assert!(fault.message.contains("example_fault"), "{fault:?}");
    assert_eq!(host.read_component(&world, entity, "Light"), light_before);
    let count = world.entities().count_spawned();
    let structural = host
        .execute_command(
            &mut world,
            "examples.structural",
            &json!({"target":entity.to_bits()}),
            None,
        )
        .expect("structural accessors");
    assert!(host.read_component(&world, entity, "Light").is_none());
    assert_eq!(
        host.read_component(&world, entity, "AudioGraphSource")
            .expect("audio graph")["parameters"]["frequency"],
        220.0
    );
    assert_eq!(world.entities().count_spawned(), count + 1);
    let spawned = structural
        .structural
        .iter()
        .find_map(|op| match op {
            ara_script::Applied::Spawn { entity } => {
                bevy_ecs::entity::Entity::try_from_bits(*entity)
            }
            _ => None,
        })
        .expect("spawn recipe produced an actor");
    for component in ["Transform", "RigidBody", "Collider", "Velocity"] {
        assert!(
            host.read_component(&world, spawned, component).is_some(),
            "spawn lost {component}"
        );
    }
    host.undo(&mut world, &structural)
        .expect("undo structural accessor changes");
    assert!(host.read_component(&world, entity, "Light").is_some());
    assert_eq!(world.entities().count_spawned(), count);
    let retired = host
        .execute_command(
            &mut world,
            "examples.retire",
            &json!({"target":entity.to_bits()}),
            None,
        )
        .expect("despawn recipe");
    assert!(world.get_entity(entity).is_err());
    host.undo(&mut world, &retired)
        .expect("restore a despawned actor including all variable values");
    assert_eq!(world.entities().count_spawned(), count);
    let restored = world.query_filtered::<bevy_ecs::entity::Entity,bevy_ecs::query::With<ara_runtime::components::Light>>()
        .single(&world).expect("restored actor");
    for name in &names {
        assert!(
            host.read_component(&world, restored, name).is_some(),
            "despawn undo lost {name}"
        );
    }
    assert_eq!(host.shutdown(), 0);
}

const RELOAD: &str = r#"namespace BindingReload;
/// <summary>A hot-reload migration probe.</summary>
[Component] public partial struct Probe {
    /// <summary>A value kept across migration.</summary>
    public int Kept;
    // fields
}
/// <summary>Writes engine components after every reload.</summary>
public sealed partial class Lamp : Behaviour {
    /// <summary>Calls observed by this behaviour.</summary>
    [Property] public partial int Seen { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick) {
        Get<Light>().Intensity = 250f;
        Get<AudioSource>().Bus = "effects";
        Get<Probe>().Kept += 1;
        Seen += 1;
    }
}
"#;

#[test]
fn accessor_components_keep_native_storage_and_values_through_l3_column_migration() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start("component-reload", &[("BindingReload", RELOAD, &[])]);
    let e = live.spawn(&[
        ("Lamp", json!({})),
        ("Probe", json!({"kept":40})),
        ("Light", json!({})),
        ("AudioSource", json!({})),
    ]);
    live.run();
    assert_eq!(live.int(e, "Probe", "kept"), 41);
    let light_id = live.host.component_id("Light");
    let audio_id = live.host.component_id("AudioSource");
    live.edit(
        "BindingReload",
        "// fields",
        "/// <summary>A newly migrated field.</summary>\n    public int Added;\n    // fields",
    );
    live.edit("BindingReload", "250f", "750f");
    live.until("L3 and new component write", |s| {
        s.count(RELOAD_L3) > 0
            && s.host
                .read_component(&s.world, e, "Light")
                .and_then(|v| v["intensity"].as_f64())
                == Some(750.0)
    });
    assert_eq!(live.host.component_id("Light"), light_id);
    assert_eq!(live.host.component_id("AudioSource"), audio_id);
    assert!(live.int(e, "Probe", "kept") > 41);
    assert_eq!(
        live.host
            .read_component(&live.world, e, "AudioSource")
            .expect("audio")["bus"],
        Value::String("effects".into())
    );
    assert_eq!(live.int(e, "Probe", "added"), 0);
    assert_eq!(live.finish(), 0);
}
