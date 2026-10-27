//! Published schemas and the runtime consume the same compiled module inventory.
mod support;

#[test]
fn published_schema_can_be_adopted_by_runtime_module_registration() {
    let _clr = support::clr_lock();
    support::build();
    let root = support::dotnet()
        .parent()
        .expect("repo")
        .join("target/script-project")
        .join(format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).expect("scratch");
    let source = root.join(ara_doc::AUTHORING_ROOT).join("Counter.cs");
    std::fs::write(&source, "/// <summary>A fixed tick counter.</summary>\n[Component] public partial struct Counter { /// <summary>Ticks.</summary>\n public int Value; }\n public static partial class Count { [Query] static void Run(ref Counter count) { count.Value++; } }").expect("source");
    let publication = ara_script::project::publish(&root, ara_script::ModuleProfile::Gameplay)
        .expect("publication");
    assert!(
        publication.diagnostics.is_empty(),
        "{:?}",
        publication.diagnostics
    );
    let specs = publication.gameplay_specs(&root).expect("specs");
    assert!(specs[0].owns_source(&source).expect("ownership"));
    let (mut schema, bindings) = support::engine();
    assert!(
        schema
            .add_json_types(publication.modules[0].schema.as_ref().expect("schema"))
            .is_empty()
    );
    let mut world = bevy_ecs::world::World::new();
    let config = ara_script::ScriptConfig::jit(support::bin("Ara.Host"));
    let mut host =
        ara_script::ScriptHost::start(config, schema, &bindings, &mut world).expect("host");
    host.load_modules(&mut world, &specs)
        .expect("adopt published schema");
    let entity = world.spawn_empty().id();
    host.insert_component(
        &mut world,
        entity,
        "Counter",
        &serde_json::json!({"value":0}),
    )
    .expect("counter");
    host.run_tick(
        &mut world,
        ara_script::abi::Tick {
            frame: 1,
            seed: 7,
            dt: 1.0 / 60.0,
            elapsed: 1.0 / 60.0,
            reserved: 0,
        },
    );
    assert_eq!(
        host.read_component(&world, entity, "Counter"),
        Some(serde_json::json!({"value":1}))
    );
    assert!(host.take_diagnostics().is_empty());
}
