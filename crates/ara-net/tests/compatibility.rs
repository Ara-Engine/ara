use ara_net::PolicyRegistry;
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn declarations() -> Value {
    json!({"$schema":"ara://schema/types/1","types":{
        "Mission":{"kind":"component","doc":"Root declaration documentation.","module":"Salvage","version":1,"fields":{
            "count":{"type":"i32","doc":"Recovered salvage.","default":0,"replicate":{"audience":"relevant"}},
            "scratch":{"type":"i32","doc":"Authority bookkeeping.","default":0}
        }}
    }})
}
fn hash(types: &Value) -> [u8; 32] {
    let mut db = SchemaDb::new();
    let diagnostics = db.add_json_types(types);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    PolicyRegistry::new(db).expect("valid protocol").hash()
}

#[test]
fn compatibility_ignores_partial_type_docs_editor_modules_and_private_state() {
    let authority = declarations();
    let mut client = authority.clone();
    client["types"]["Mission"]["doc"] = json!("A helper partial's documentation.");
    client["types"]["Mission"]["fields"]["count"]["doc"] = json!("Localized inspector tooltip.");
    client["types"]["Mission"]["fields"]["scratch"]["default"] = json!(42);
    client["types"]["EditorOnly"] = json!({"kind":"component","doc":"Client editor state.","fields":{"selected":{"type":"bool","doc":"Selection.","default":false}}});
    assert_eq!(
        hash(&authority),
        hash(&client),
        "presentation metadata and authority-only state do not change the wire contract"
    );
}

#[test]
fn compatibility_still_rejects_changed_wire_types_versions_and_policies() {
    let original = declarations();
    let base = hash(&original);
    for (path, value) in [
        ("/types/Mission/version", json!(2)),
        ("/types/Mission/fields/count/type", json!("u32")),
        (
            "/types/Mission/fields/count/replicate/audience",
            json!("owner"),
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).expect("fixture path") = value;
        assert_ne!(base, hash(&changed), "wire change {path}");
    }
    let mut quantized = original.clone();
    quantized["types"]["Mission"]["fields"]["count"]["replicate"]["quantize"] = json!(1);
    assert_ne!(base, hash(&quantized));
}

#[test]
fn compatibility_tracks_reachable_enum_and_collection_shapes_in_stable_order() {
    let mut original = declarations();
    original["types"]["Cargo"] = json!({"kind":"struct","doc":"One cargo item.","version":1,"fields":{
        "kind":{"type":"CargoKind","doc":"Cargo category."},"amount":{"type":"u32","doc":"Stack size."}
    }});
    original["types"]["CargoKind"] = json!({"kind":"enum","doc":"Cargo categories.","variants":{"cell":{"doc":"Battery."},"radio":{"doc":"Radio."}}});
    original["types"]["Mission"]["fields"]["cargo"] =
        json!({"type":"list<Cargo>","doc":"Public cargo.","replicate":{"audience":"relevant"}});
    let base = hash(&original);
    let mut metadata = original.clone();
    metadata["types"]["Cargo"]["doc"] = json!("Client tooltip.");
    metadata["types"]["CargoKind"]["variants"]["cell"]["doc"] = json!("Localized battery tooltip.");
    assert_eq!(base, hash(&metadata));
    for (path, value) in [
        ("/types/Cargo/version", json!(2)),
        ("/types/Cargo/fields/amount/type", json!("u64")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).expect("fixture path") = value;
        assert_ne!(base, hash(&changed));
    }
    original["types"]["CargoKind"]["variants"]["circuit"] = json!({"doc":"Circuit board."});
    assert_ne!(base, hash(&original));
}
