//! Initial open publishes C# schemas before any scene validation or Play process.
use ara_script::{ModuleProfile, project};
use std::path::PathBuf;

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
    std::fs::write(root.join("ara.project.json"), "{}").expect("marker");
    let source = root.join(ara_doc::AUTHORING_ROOT).join("Armour.cs");
    std::fs::write(&source, "using Ara; /// <summary>Protection.</summary>\n[Component] public partial struct Armour { /// <summary>Points.</summary>\n public float Points; }").expect("source");
    let opened = ara_play_host::open_project(&root).expect("open");
    assert!(opened.diags.is_empty(), "{:?}", opened.diags);
    assert!(
        opened.db.get("Armour").is_some(),
        "schema must exist before Host loads scenes"
    );
    let schema = root.join(".ara/scripts/game/game.types.json");
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
