//! Published schemas and the runtime consume the same compiled module inventory.
mod support;

#[test]
fn relative_project_roots_resolve_once_and_private_module_paths_are_canonical() {
    if std::env::var_os("ARA_RELATIVE_MODULE_CHILD").is_some() {
        let root = std::path::Path::new("project");
        let inventory = ara_script::project::inventory(root).expect("relative inventory");
        assert_eq!(
            inventory.sources.len(),
            1,
            "relative inventory root must not resolve twice"
        );
        let nested = root.join(ara_doc::AUTHORING_ROOT).join("packages/nested");
        std::fs::create_dir_all(&nested).expect("authored packages");
        std::fs::rename(
            root.join(ara_doc::AUTHORING_ROOT).join("Counter.cs"),
            nested.join("Counter.cs"),
        )
        .expect("move into authored packages");
        let nested_inventory =
            ara_script::project::inventory(root).expect("nested source inventory");
        assert_eq!(
            nested_inventory.sources.len(),
            1,
            "the Rust empty-project guard must include authored packages"
        );
        assert_eq!(
            nested_inventory.sources[0].path,
            "packages/nested/Counter.cs"
        );
        let compiled = ara_script::project::compile(root, ara_script::ModuleProfile::Gameplay)
            .expect("relative compile");
        assert_eq!(compiled.modules[0].result, "compiled");
        let published = ara_script::project::publish(root, ara_script::ModuleProfile::Gameplay)
            .expect("relative publication");
        let specs = published.gameplay_specs(root).expect("private modules");
        assert_eq!(
            specs[0].dir,
            std::fs::canonicalize(&specs[0].dir).expect("canonical generated root")
        );
        let generated = specs[0].dir.join("game.types.json");
        assert!(generated.is_absolute());
        std::env::set_current_dir("elsewhere").expect("change only this isolated test child's cwd");
        assert!(
            std::fs::read_to_string(generated)
                .expect("tracked schema stays reachable")
                .contains("Counter")
        );
        return;
    }
    support::build();
    let base = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            support::dotnet()
                .parent()
                .expect("repo")
                .join("target/relative-script-root")
        });
    let parent = base.join(format!("relative-{}", std::process::id()));
    std::fs::create_dir_all(parent.join("project").join(ara_doc::AUTHORING_ROOT)).expect("project");
    std::fs::create_dir_all(parent.join("elsewhere")).expect("other cwd");
    std::fs::write(parent.join("project").join(ara_doc::AUTHORING_ROOT).join("Counter.cs"), "/// <summary>A counter.</summary>\n[Component] public partial struct Counter { /// <summary>Ticks.</summary>\n public int Value; }").expect("source");
    let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .current_dir(parent)
        .args([
            "--exact",
            "relative_project_roots_resolve_once_and_private_module_paths_are_canonical",
            "--nocapture",
        ])
        .env("ARA_RELATIVE_MODULE_CHILD", "1")
        .output()
        .expect("isolated relative-root caller");
    assert!(
        child.status.success(),
        "{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

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
