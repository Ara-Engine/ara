//! Published schemas and the runtime consume the same compiled module inventory.
mod support;

fn marker(root: &std::path::Path) {
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
        ),
    )
    .expect("current project marker");
}

#[test]
fn source_presence_hint_never_traverses_directory_links() {
    let target = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            support::dotnet()
                .parent()
                .expect("repo")
                .join("target/script-project")
        });
    let scratch = target.join(format!("linked-sources-{}", std::process::id()));
    let project = scratch.join("project");
    let external = scratch.join("outside");
    let authored = project.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(&authored).expect("project");
    marker(&project);
    std::fs::create_dir_all(&external).expect("external source directory");
    std::fs::write(external.join("Outside.cs"), "class Outside { }").expect("outside source");
    let link = authored.join("linked");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&external)
            .creation_flags(0x0800_0000)
            .output()
            .expect("create junction");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &link).expect("create directory symlink");
    assert!(
        !ara_script::project::has_sources(&project).expect("presence hint"),
        "directory links must not contribute authored sources"
    );
}

#[test]
fn empty_project_publication_does_not_initialize_derived_state() {
    let target = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            support::dotnet()
                .parent()
                .expect("repo")
                .join("target/script-project")
        });
    let root = target.join(format!("empty-publication-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("scratch");
    marker(&root);
    for inventory in [
        ara_script::project::inventory(&root),
        ara_script::project::compile(&root, ara_script::ModuleProfile::Gameplay),
        ara_script::project::publish(&root, ara_script::ModuleProfile::Gameplay),
    ] {
        let inventory = inventory.expect("empty project");
        assert!(inventory.modules.is_empty());
        assert!(inventory.diagnostics.is_empty());
    }
    assert!(
        !root.join(".ara").exists(),
        "a project without C# must not initialize compiler state"
    );
}

#[test]
fn migrated_outpost_sample_compiles_and_ticks_without_shipping_its_editor_extension() {
    let _clr = support::clr_lock();
    support::build();
    let target = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            support::dotnet()
                .parent()
                .expect("repo")
                .join("target/script-project")
        });
    let root = target.join(format!("migrated-outpost-{}", std::process::id()));
    let module = root.join(ara_doc::AUTHORING_ROOT).join("Outpost");
    std::fs::create_dir_all(module.join("editor")).expect("migrated source folders");
    marker(&root);
    // The migration owner moves the legacy sources and preserves this module identity.
    for source in ["Door.cs", "Motion.cs", "Vitals.cs"] {
        std::fs::copy(
            support::dotnet()
                .join("samples/Outpost.Sample")
                .join(source),
            module.join(source),
        )
        .expect("legacy sample source");
    }
    std::fs::write(
        module.join("outpost.sample.module.json"),
        ara_json::to_canonical_string(
            &serde_json::json!({"name":"Outpost.Sample","references":[],"hot":false}),
        ),
    )
    .expect("migrated manifest");
    std::fs::write(module.join("editor/Tools.cs"), "using Ara.Editor; public sealed class Tools : IEditorExtension { public void Register(EditorRegistry registry) => registry.RegisterCommand(\"outpost.inspect\", \"Inspect Outpost\", context => context.Scene.Find(\"Door\")); }").expect("editor extension");
    let published = ara_script::project::publish(&root, ara_script::ModuleProfile::Gameplay)
        .expect("publication");
    assert!(
        published.diagnostics.is_empty(),
        "{:?}",
        published.diagnostics
    );
    let specs = published.gameplay_specs(&root).expect("player modules");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "Outpost.Sample");
    assert!(!specs[0].hot);
    let gameplay = published
        .modules
        .iter()
        .find(|m| m.name == "Outpost.Sample")
        .expect("gameplay");
    assert_eq!(gameplay.sources.len(), 3);
    let assembly = std::fs::read(root.join(&gameplay.assembly_path)).expect("player assembly");
    assert!(
        !assembly
            .windows(b"Ara.Editor".len())
            .any(|bytes| bytes == b"Ara.Editor")
    );
    let (mut schema, bindings) = support::engine();
    assert!(
        schema
            .add_json_types(gameplay.schema.as_ref().expect("published schema"))
            .is_empty()
    );
    let mut world = bevy_ecs::world::World::new();
    let mut host = ara_script::ScriptHost::start(
        ara_script::ScriptConfig::jit(support::bin("Ara.Host")),
        schema,
        &bindings,
        &mut world,
    )
    .expect("host");
    host.load_modules(&mut world, &specs)
        .expect("migrated sample loads");
    let door = world.spawn(support::EcsTransform::default()).id();
    host.insert_component(
        &mut world,
        door,
        "Door",
        &serde_json::json!({"openAngle":90,"isOpen":true}),
    )
    .expect("door behaviour");
    host.run_tick(
        &mut world,
        ara_script::abi::Tick {
            frame: 1,
            seed: 7,
            dt: 0.1,
            elapsed: 0.1,
            reserved: 0,
        },
    );
    assert_eq!(
        world
            .get::<support::EcsTransform>(door)
            .expect("transform")
            .rotation[1],
        9.0
    );
    assert!(host.take_diagnostics().is_empty());
    assert!(!module.join("Outpost.Sample.csproj").exists());
    assert_eq!(host.shutdown(), 0);
}

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
    marker(&parent.join("project"));
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
    marker(&root);
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
