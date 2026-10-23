#![allow(clippy::unwrap_used)]
use ara_save::{tools, *};
use ara_schema::{Schema, SchemaDb};
use ara_sim::bevy_ecs;
use ara_sim::{
    EditWorld, Identity, Session, SimContext, SimRegistry, TickConfig, bevy_ecs::prelude::*,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Persistent service cursor used to check resource presence/absence handling.
#[derive(Resource, Schema, Serialize, Deserialize)]
struct ServiceCursor {
    /// Last serviced event.
    value: u64,
}

fn schema(version: u32) -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    db.register::<ServiceCursor>();
    assert!(db.add_json_types(&json!({"$schema":"ara://schema/types/1","types":{"Stats":{"kind":"component","doc":"Persistent gameplay state.","version":version,"fields":{
        "hp":{"type":"i32","doc":"Current hit points.","default":100},
        "balance":{"type":"f64","doc":"Exact float storage test."},
        "items":{"type":"list<string>","doc":"Inventory item names."}
    }},"Presentation":{"kind":"component","doc":"Excluded presentation state.","fields":{"flash":{"type":"f32","doc":"Hit flash intensity."}}}}})).is_empty());
    db
}
fn policy(db: &SchemaDb) -> SaveSchema {
    SaveSchema::new(
        db.clone(),
        [
            PersistDeclaration {
                type_name: "Stats".into(),
                storage: Storage::Component,
                persist: true,
            },
            PersistDeclaration {
                type_name: "Presentation".into(),
                storage: Storage::Component,
                persist: false,
            },
            PersistDeclaration {
                type_name: "ServiceCursor".into(),
                storage: Storage::Resource,
                persist: true,
            },
        ],
    )
    .unwrap()
}
fn session(db: &SchemaDb) -> Session {
    let text = r#"{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{"e_0001":{"name":"Player","Stats":{"hp":73,"balance":-0.0,"items":["key","salvage"]},"Presentation":{"flash":0.2}}}}"#;
    let text = if db.get("Stats").unwrap().field("health").is_some() {
        text.replace("\"hp\"", "\"health\"").replacen(
            "\"records\"",
            "\"versions\":{\"Stats\":2},\"records\"",
            1,
        )
    } else {
        text.into()
    };
    let read = ara_doc::read(&text, "level.scene", db, None);
    assert!(read.diags.iter().all(|d| !d.is_error()), "{:?}", read.diags);
    let mut registry = SimRegistry::new(db.clone()).unwrap();
    registry.register_resource::<ServiceCursor>().unwrap();
    let edit = EditWorld::new(registry, read.doc.unwrap()).unwrap();
    let mut session = Session::local(edit, TickConfig::default(), 42, Default::default()).unwrap();
    session
        .world_mut()
        .insert_resource(ServiceCursor { value: u64::MAX });
    session
        .add_system("draw", |mut context: ResMut<SimContext>| {
            context.rng("loot", "e_0001").next_u64();
        })
        .unwrap();
    session
}
fn metadata(slot: &str) -> SlotMetadata {
    SlotMetadata {
        slot: slot.into(),
        timestamp_millis: 1_792_728_000_000,
        playtime_millis: 123_456,
        thumbnail: None,
        label: "Outpost checkpoint".into(),
    }
}

#[test]
fn save_load_roundtrip_restores_persistent_and_full_sim_hash_at_same_tick() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    session.step().unwrap();
    session.step().unwrap();
    let before = session.state_hash().unwrap();
    let game = SaveGame::capture(&schema, &session, metadata("quick")).unwrap();
    assert!(
        !game.state.entities["e_0001"]
            .components
            .contains_key("Presentation")
    );
    let bytes = encode(&game, Limits::default()).unwrap();
    let decoded = decode(&bytes, &schema, Limits::default()).unwrap();
    assert_eq!(decoded.original_state_hash, game.state_hash);
    assert_eq!(decoded.game.state_hash, game.state_hash);
    session
        .write_runtime_component("e_0001", "Stats", &json!({"hp":1}))
        .unwrap();
    session.step().unwrap();
    schema.restore(&mut session, &decoded.game.state).unwrap();
    assert_eq!(session.state_hash().unwrap(), before);
    assert_eq!(
        schema.capture(&session).unwrap().state_hash().unwrap(),
        game.state_hash
    );
    assert_eq!(session.world().resource::<ServiceCursor>().value, u64::MAX);
    assert_eq!(session.tick(), 2);
    let mut matching = self::session(&db);
    matching.step().unwrap();
    matching.step().unwrap();
    matching.step().unwrap();
    session.step().unwrap();
    assert_eq!(
        session.state_hash().unwrap(),
        matching.state_hash().unwrap()
    );
}

#[test]
fn v1_to_v2_reuses_schema_upgrade_hook_and_validates_result() {
    let old_db = schema(1);
    let old_schema = policy(&old_db);
    let session = session(&old_db);
    let old = SaveGame::capture(&old_schema, &session, metadata("migration")).unwrap();
    let bytes = encode(&old, Limits::default()).unwrap();
    let mut new_db = SchemaDb::new();
    ara_doc::register(&mut new_db);
    new_db.register::<ServiceCursor>();
    let mut desc = old_db.get("Stats").unwrap().clone();
    desc.version = 2;
    desc.upgrade = Some(|from, value| {
        if from != 1 {
            return Err("unexpected version".into());
        }
        let fields = value.as_object_mut().ok_or("expected object")?;
        let old = fields.remove("hp").ok_or("missing hp")?;
        fields.insert("health".into(), old);
        Ok(())
    });
    let ara_schema::Shape::Struct { fields, .. } = &mut desc.shape else {
        panic!("struct")
    };
    fields[0].name = "health".into();
    let mut regen = ara_schema::FieldDesc::new(
        "regen",
        ara_schema::ValueType::F32,
        "New defaulted regeneration fraction.",
    );
    regen.default = Some(json!(0.1));
    fields.push(regen);
    new_db.add(desc).unwrap();
    new_db
        .add(old_db.get("Presentation").unwrap().clone())
        .unwrap();
    let new_schema = policy(&new_db);
    let decoded = decode(&bytes, &new_schema, Limits::default()).unwrap();
    assert_eq!(decoded.game.versions["Stats"], 2);
    assert_eq!(
        decoded.game.state.entities["e_0001"].components["Stats"]["health"],
        73
    );
    assert_ne!(decoded.original_state_hash, decoded.game.state_hash);
    let mut current = self::session(&new_db);
    new_schema
        .restore(&mut current, &decoded.game.state)
        .unwrap();
    assert_eq!(
        new_schema.capture(&current).unwrap().state_hash().unwrap(),
        decoded.game.state_hash
    );
    assert!(
        decoded
            .diagnostics
            .iter()
            .any(|d| d.code == ara_diag::codes::UPGRADED)
    );
    assert!(
        decode(
            &encode(&decoded.game, Limits::default()).unwrap(),
            &old_schema,
            Limits::default()
        )
        .is_err()
    );
}

#[test]
fn corruption_uses_backup_and_atomic_repair_keeps_verified_generation() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let dir = tempfile::tempdir().unwrap();
    let store = SlotStore::new(dir.path(), Authority::Server, Limits::default());
    let first = SaveGame::capture(&schema, &session, metadata("quick")).unwrap();
    let first_info = store.write(&first, &schema, None).unwrap();
    session
        .write_runtime_component("e_0001", "Stats", &json!({"hp":25}))
        .unwrap();
    let second = SaveGame::capture(&schema, &session, metadata("quick")).unwrap();
    store
        .write(&second, &schema, Some(first_info.file_hash))
        .unwrap();
    let path = dir.path().join("quick.ara-save");
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    let loaded = store.load("quick", &schema).unwrap();
    assert!(loaded.recovered);
    assert_eq!(loaded.save.game.state_hash, first.state_hash);
    assert!(loaded.diagnostics.iter().any(|d| d.code == codes::BACKUP));
    store.restore_backup("quick", &schema).unwrap();
    let repaired = store.load("quick", &schema).unwrap();
    assert!(!repaired.recovered);
    assert_eq!(repaired.save.file_hash, first_info.file_hash);
    session
        .write_runtime_component("e_0001", "Stats", &json!({"hp":0}))
        .unwrap();
    tools::apply_load(&store, &schema, &mut session, &loaded, true).unwrap();
    assert_eq!(
        session.component("e_0001", "Stats").unwrap().unwrap()["hp"],
        73
    );
    assert_eq!(store.list(&schema).unwrap().slots.len(), 1);
}

#[test]
fn saves_diff_has_structured_field_paths_and_missing_differs_from_null() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let before = SaveGame::capture(&schema, &session, metadata("before")).unwrap();
    session
        .write_runtime_component(
            "e_0001",
            "Stats",
            &json!({"hp":12,"items":["key","medkit"]}),
        )
        .unwrap();
    let after = SaveGame::capture(&schema, &session, metadata("after")).unwrap();
    let fields = diff(&before, &after).unwrap();
    assert!(
        fields
            .iter()
            .any(|d| d.path == "/entities/e_0001/components/Stats/hp"
                && d.before == Some(json!(73))
                && d.after == Some(json!(12)))
    );
    assert!(
        fields
            .iter()
            .any(|d| d.path == "/entities/e_0001/components/Stats/items/1"
                && d.after == Some(json!("medkit")))
    );
    assert!(fields.iter().all(|d| !d.path.contains("metadata")));
    let dir = tempfile::tempdir().unwrap();
    let store = SlotStore::new(dir.path(), Authority::Local, Limits::default());
    store.write(&before, &schema, None).unwrap();
    store.write(&after, &schema, None).unwrap();
    let tool = tools::diff_slots(
        &store,
        &schema,
        tools::DiffParams {
            before: "before".into(),
            after: "after".into(),
        },
    )
    .unwrap();
    assert_eq!(tool.fields, fields);
}

#[test]
fn authority_conflicts_traversal_and_apply_policy_are_enforced() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let dir = tempfile::tempdir().unwrap();
    let game = SaveGame::capture(&schema, &session, metadata("slot1")).unwrap();
    let server = SlotStore::new(dir.path(), Authority::Server, Limits::default());
    let client = SlotStore::new(dir.path(), Authority::Client, Limits::default());
    assert_eq!(
        client.write(&game, &schema, None).unwrap_err().code,
        codes::AUTHORITY
    );
    assert!(
        tools::prepare_write(
            &client,
            &schema,
            &session,
            tools::WriteParams {
                metadata: metadata("slot1"),
                expected_hash: None
            }
        )
        .is_err()
    );
    let info = server.write(&game, &schema, None).unwrap();
    assert!(
        server
            .write(&game, &schema, Some(StateHash::of_bytes(b"stale")))
            .is_err()
    );
    assert_eq!(
        server.load("slot1", &schema).unwrap().save.file_hash,
        info.file_hash
    );
    for invalid in ["../escape", "C:\\escape", "CON", "a.b", "", "slot/2"] {
        assert!(server.load(invalid, &schema).is_err());
    }
    let loaded = server.load("slot1", &schema).unwrap();
    let hash = session.state_hash().unwrap();
    assert!(tools::apply_load(&server, &schema, &mut session, &loaded, false).is_err());
    assert_eq!(session.state_hash().unwrap(), hash);
    assert!(tools::apply_load(&client, &schema, &mut session, &loaded, true).is_err());
}

#[test]
fn malformed_binary_lengths_checksums_and_trailing_data_fail() {
    let db = schema(1);
    let schema = policy(&db);
    let game = SaveGame::capture(&schema, &session(&db), metadata("slot1")).unwrap();
    let bytes = encode(&game, Limits::default()).unwrap();
    for length in [0, 7, 59, bytes.len() - 1] {
        assert!(decode(&bytes[..length], &schema, Limits::default()).is_err());
    }
    let mut bad = bytes.clone();
    bad[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(decode(&bad, &schema, Limits::default()).is_err());
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(decode(&bad, &schema, Limits::default()).is_err());
    assert!(
        decode(
            &bytes,
            &schema,
            Limits {
                file_bytes: bytes.len() - 1,
                decoded_bytes: 256 * 1024 * 1024
            }
        )
        .is_err()
    );
    assert!(
        decode(
            &bytes,
            &schema,
            Limits {
                file_bytes: bytes.len(),
                decoded_bytes: 4
            }
        )
        .is_err()
    );
    let mut mismatched = game;
    mismatched.state_hash = StateHash::default();
    assert!(encode(&mismatched, Limits::default()).is_err());
}

#[test]
fn resources_absence_and_runtime_entities_restore_without_presentation_replacement() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let entity = session
        .world_mut()
        .spawn(Identity("e_runtime1".into()))
        .id();
    session
        .write_runtime_component("e_runtime1", "Stats", &json!({"hp":9}))
        .unwrap();
    session.world_mut().remove_resource::<ServiceCursor>();
    let game = SaveGame::capture(&schema, &session, metadata("entities")).unwrap();
    session.world_mut().despawn(entity);
    session
        .world_mut()
        .insert_resource(ServiceCursor { value: 5 });
    session
        .write_runtime_component("e_0001", "Presentation", &json!({"flash":0.9}))
        .unwrap();
    schema.restore(&mut session, &game.state).unwrap();
    assert_eq!(
        session.component("e_runtime1", "Stats").unwrap().unwrap()["hp"],
        9
    );
    assert!(session.world().get_resource::<ServiceCursor>().is_none());
    let flash = session
        .component("e_0001", "Presentation")
        .unwrap()
        .unwrap()["flash"]
        .as_f64()
        .unwrap();
    assert!((flash - 0.9).abs() < 1e-6);
}

#[test]
fn invalid_restore_is_atomic_and_future_policy_is_rejected() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let before = session.state_hash().unwrap();
    let mut state = schema.capture(&session).unwrap();
    state
        .entities
        .get_mut("e_0001")
        .unwrap()
        .components
        .insert("Stats".into(), json!({"hp":"wrong"}));
    assert!(schema.restore(&mut session, &state).is_err());
    assert_eq!(session.state_hash().unwrap(), before);
    let mut game = SaveGame::capture(&schema, &session, metadata("future")).unwrap();
    game.versions.insert("Stats".into(), 99);
    assert!(
        decode(
            &encode(&game, Limits::default()).unwrap(),
            &schema,
            Limits::default()
        )
        .is_err()
    );
}

#[test]
fn save_tool_params_are_reflected_and_examples_deserialize_without_authority_params() {
    let mut db = schema(1);
    let catalog = tools::register(&mut db);
    assert!(
        db.verify().iter().all(|d| !d.is_error()),
        "{:?}",
        db.verify()
    );
    assert!(catalog.iter().any(|tool| tool.name == "save.write"));
    assert!(catalog.iter().any(|tool| tool.name == "save.diff"));
    let request = json!({"metadata":metadata("quick"),"expectedHash":null});
    let _: tools::WriteParams = serde_json::from_value(request).unwrap();
    let illegal = json!({"slot":"quick","authority":"server"});
    assert!(serde_json::from_value::<tools::LoadParams>(illegal).is_err());
}

#[test]
fn persistent_hierarchy_restores_and_excluded_children_survive_parent_despawn() {
    let db = schema(1);
    let schema = policy(&db);
    let mut session = session(&db);
    let root = session.entity("e_0001").unwrap();
    let anchor = session.world_mut().spawn(Identity("e_anchor1".into())).id();
    session.world_mut().entity_mut(root).insert(ChildOf(anchor));
    let state = schema.capture(&session).unwrap();
    assert!(state.entities["e_anchor1"].components.is_empty());
    session.world_mut().entity_mut(root).remove::<ChildOf>();
    schema.restore(&mut session, &state).unwrap();
    assert_eq!(
        session.world().get::<ChildOf>(root).unwrap().parent(),
        anchor
    );
    let runtime = session
        .world_mut()
        .spawn(Identity("e_exclude1".into()))
        .id();
    session
        .world_mut()
        .entity_mut(runtime)
        .insert(ChildOf(root));
    let mut removed = state;
    removed.entities.clear();
    schema.restore(&mut session, &removed).unwrap();
    assert!(session.world().get_entity(runtime).is_ok());
    assert!(session.world().get::<ChildOf>(runtime).is_none());
}
