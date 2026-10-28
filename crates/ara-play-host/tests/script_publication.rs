//! Initial open publishes C# schemas before any scene validation or Play process.
use ara_script::{ModuleProfile, project};
use std::path::PathBuf;

#[test]
fn relative_open_tracks_absolute_canonical_schema_files_across_cwd_changes() {
    if std::env::var_os("ARA_RELATIVE_SCHEMA_CHILD").is_some() {
        let project =
            ara_play_host::open_project(std::path::Path::new("project")).expect("relative open");
        assert!(project.diags.is_empty(), "{:?}", project.diags);
        assert_eq!(project.type_files.len(), 1);
        let file = &project.type_files[0];
        assert_eq!(
            *file,
            std::fs::canonicalize(file).expect("canonical schema output")
        );
        std::env::set_current_dir("elsewhere").expect("isolated child cwd");
        assert!(
            std::fs::read_to_string(file)
                .expect("tracked schema")
                .contains("Counter")
        );
        return;
    }
    let base = std::env::var_os("ARA_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/relative-schema-root")
        });
    let parent = base.join(format!("relative-open-{}", std::process::id()));
    std::fs::create_dir_all(parent.join("project").join(ara_doc::AUTHORING_ROOT)).expect("project");
    std::fs::create_dir_all(parent.join("elsewhere")).expect("other cwd");
    std::fs::write(
        parent.join("project/ara.project.json"),
        ara_json::to_canonical_string(
            &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
        ),
    )
    .expect("marker");
    std::fs::write(parent.join("project").join(ara_doc::AUTHORING_ROOT).join("Counter.cs"), "/// <summary>A counter.</summary>\n[Component] public partial struct Counter { /// <summary>Ticks.</summary>\n public int Value; }").expect("source");
    let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .current_dir(parent)
        .args([
            "--exact",
            "relative_open_tracks_absolute_canonical_schema_files_across_cwd_changes",
            "--nocapture",
        ])
        .env("ARA_RELATIVE_SCHEMA_CHILD", "1")
        .output()
        .expect("isolated caller");
    assert!(
        child.status.success(),
        "{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

#[test]
fn fresh_open_publishes_and_repairs_private_schemas_before_validation() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/script-publication")
        .join(format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).expect("scratch");
    std::fs::write(root.join("ara.project.json"), ara_json::to_canonical_string(&serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"main.scene"}))).expect("marker");
    let source = root.join(ara_doc::AUTHORING_ROOT).join("Armour.cs");
    std::fs::write(&source, "using Ara; /// <summary>Protection.</summary>\n[Component] public partial struct Armour { /// <summary>Points.</summary>\n public float Points; }").expect("source");
    let opened = ara_play_host::open_project(&root).expect("open");
    assert!(opened.diags.is_empty(), "{:?}", opened.diags);
    assert!(
        opened.db.get("Armour").is_some(),
        "schema must exist before Host loads scenes"
    );
    let schema = std::fs::canonicalize(root.join(".ara/scripts/game/game.types.json"))
        .expect("canonical schema file");
    assert_eq!(opened.type_files.as_slice(), std::slice::from_ref(&schema));
    assert!(root.join(".ara/scripts/game/game.csproj").is_file());
    let cached = project::publish(&root, ModuleProfile::Gameplay).expect("cache");
    assert_eq!(cached.modules[0].result, "cached");
    let specs = cached.gameplay_specs(&root).expect("specs");
    assert_eq!(specs[0].name, "game");
    assert_eq!(
        std::fs::canonicalize(&specs[0].dir).expect("module"),
        std::fs::canonicalize(&root)
            .expect("root")
            .join(".ara/scripts/game")
    );
    std::fs::write(&schema, "corrupt").expect("simulate damaged derived output");
    let repaired = project::publish(&root, ModuleProfile::Gameplay).expect("repair");
    assert_eq!(repaired.modules[0].result, "compiled");
    assert!(
        repaired.modules[0].schema.as_ref().expect("schema")["types"]
            .get("Armour")
            .is_some()
    );
    std::fs::write(&source, "invalid C#").expect("break compilation");
    let broken = ara_play_host::open_project(&root).expect("diagnosable open");
    assert!(
        broken
            .diags
            .iter()
            .any(|d| d.severity == ara_diag::Severity::Error)
    );
    assert!(
        broken.db.get("Armour").is_none(),
        "failed compilation must not silently load stale schema"
    );
    assert!(
        !source
            .parent()
            .expect("parent")
            .join("game.csproj")
            .exists()
    );
}
