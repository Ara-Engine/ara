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
fn component_indices_and_nested_enum_variants_participate_in_the_hash() {
    let mut db = sample_db();
    let hash = |db: &SchemaDb| {
        ara_script::abi::layout_hash(&csgen::engine_layouts(db, &["Door"]).expect("layouts"))
    };
    let before = hash(&db);
    let mut hinge = db.remove("Hinge").expect("nested type");
    if let ara_schema::Shape::Struct { role, .. } = &mut hinge.shape {
        *role = ara_schema::StructRole::Component;
    }
    db.add(hinge).expect("same fields, now also a component");
    assert_ne!(
        before,
        hash(&db),
        "component indices are ABI even if reachable fields are unchanged"
    );
    let before = hash(&db);
    let mut state = db.remove("DoorState").expect("enum");
    if let ara_schema::Shape::Enum { variants } = &mut state.shape {
        variants.swap(0, 1);
    }
    db.add(state).expect("reordered enum");
    assert_ne!(before, hash(&db), "wire enum ordering is fingerprinted");
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
    let db = ara_runtime::schema(&[]).expect("engine schema");
    let bindings = ara_script::EngineBindings::builtin();
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

#[test]
fn every_component_and_variable_field_has_a_generated_binding() {
    let db = ara_runtime::schema(&[]).expect("engine schema");
    let bindings = ara_script::EngineBindings::builtin();
    let files = csgen::csharp(&db, &bindings.names()).expect("generates every engine component");
    let text = &files
        .iter()
        .find(|f| f.name == "Components.g.cs")
        .expect("components")
        .text;
    for component in db.components() {
        assert!(
            text.contains(&format!(
                "struct {} : IComponent<{}>",
                component.name, component.name
            )),
            "missing {}",
            component.name
        );
        for f in component.fields() {
            assert!(
                text.contains(&format!("w.WritePropertyName(\"{}\")", f.name)),
                "missing {}.{}",
                component.name,
                f.name
            );
        }
    }
    let mut variable = SchemaDb::new();
    assert!(variable.add_json_types(&json!({"$schema":"ara://schema/types/1","types":{"Label":{"kind":"component","doc":"Label.","fields":{"text":{"type":"string","doc":"Text."}}}}})).is_empty());
    assert!(
        csgen::csharp(&variable, &[])
            .expect("string accessor binding")
            .iter()
            .any(|f| f.text.contains("struct Label : IComponent<Label>"))
    );
}

#[test]
fn offline_shipping_callers_get_the_same_complete_binding_identity() {
    let full = ara_runtime::schema(&[]).expect("engine schema");
    let mut base = SchemaDb::new();
    ara_components::register(&mut base);
    let full_layouts = csgen::engine_layouts(&full, &["Transform"]).expect("runtime layouts");
    let offline_layouts = csgen::engine_layouts(&base, &["Transform"]).expect("shipping layouts");
    assert_eq!(
        ara_script::abi::layout_hash(&full_layouts),
        ara_script::abi::layout_hash(&offline_layouts)
    );
    assert_eq!(
        csgen::csharp(&full, &["Transform"]).expect("runtime bindings"),
        csgen::csharp(&base, &["Transform"]).expect("offline bindings")
    );
}
