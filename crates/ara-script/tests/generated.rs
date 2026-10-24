//! The Rust → C# generator: snapshot of a representative schema, and the checked-in
//! `Ara.Runtime/Generated` files staying fresh.

use std::path::Path;

use ara_schema::SchemaDb;
use ara_script::csgen;
use serde_json::json;

/// A schema exercising every blittable field kind and metadata attribute.
fn sample_db() -> SchemaDb {
    let mut db = SchemaDb::new();
    let problems = db.add_json_types(&json!({
        "$schema": "ara://schema/types/1",
        "types": {
            "DoorState": { "kind": "enum", "doc": "Where a door is in its swing.", "variants": {
                "closed": { "doc": "Fully shut." }, "opening": { "doc": "Swinging open." } } },
            "Hinge": { "kind": "struct", "doc": "A hinge axis.", "fields": {
                "axis": { "type": "vec3", "doc": "Rotation axis.", "default": [0, 1, 0] },
                "limit": { "type": "f32", "doc": "Swing limit.", "unit": "deg", "range": [0, 180] } } },
            "Door": { "kind": "component", "doc": "A door that swings on a hinge.", "module": "game", "fields": {
                "state": { "type": "DoorState", "doc": "Current state." },
                "hinge": { "type": "Hinge", "doc": "The hinge it swings on." },
                "speed": { "type": "f32", "doc": "Swing speed.", "unit": "deg/s", "range": [0, null],
                    "default": 90, "replicate": { "audience": "relevant", "quantize": 0.5, "interpolate": "linear" } },
                "locked": { "type": "bool", "doc": "Locked doors ignore use.", "aliases": ["isLocked"] },
                "owner": { "type": "entity", "doc": "Who may unlock it.", "replicate": { "audience": "owner" } },
                "tint": { "type": "color", "doc": "Paint color." } } }
        }
    }));
    assert!(problems.is_empty(), "{problems:?}");
    db
}

#[test]
fn components_snapshot() {
    let db = sample_db();
    let files = csgen::csharp(&db, &["Door"]).expect("generates");
    let components = files
        .iter()
        .find(|f| f.name == "Components.g.cs")
        .expect("components file");
    insta::assert_snapshot!(components.text);
}

#[test]
fn non_blittable_components_are_reported() {
    let mut db = SchemaDb::new();
    let problems = db.add_json_types(&json!({
        "$schema": "ara://schema/types/1",
        "types": { "Label": { "kind": "component", "doc": "A label.", "fields": {
            "text": { "type": "string", "doc": "Text." } } } }
    }));
    assert!(problems.is_empty());
    let err = csgen::csharp(&db, &["Label"]).expect_err("strings are not blittable");
    assert_eq!(err.code, ara_script::codes::NOT_BLITTABLE);
    assert!(csgen::blittable_components(&db).is_empty());
}

/// `Ara.Runtime/Generated` must match the columns the runtime actually binds.
/// Run with `ARA_UPDATE_GENERATED=1` to rewrite it after an ABI or schema change.
#[test]
fn checked_in_bindings_are_fresh() {
    let mut db = SchemaDb::new();
    ara_components::register(&mut db);
    let bindings = ara_script::engine_bindings();
    let engine = bindings.names();
    let files = csgen::csharp(&db, &engine).expect("generates");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet/src/Ara.Runtime/Generated");
    if std::env::var_os("ARA_UPDATE_GENERATED").is_some() {
        csgen::write(&dir, &files).expect("write generated files");
    }
    let stale = csgen::stale(&dir, &files);
    assert!(
        stale.is_empty(),
        "stale generated C# (rerun with ARA_UPDATE_GENERATED=1): {stale:?}"
    );
}
