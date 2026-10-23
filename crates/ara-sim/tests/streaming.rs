#![allow(clippy::unwrap_used)]
use ara_components::Transform;
use ara_doc::{AssetId, Document};
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, Session, SimRegistry, StateHash, TickConfig};
use serde_json::json;

fn setup() -> (SimRegistry, Session) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let registry = SimRegistry::new(db).unwrap();
    let scene = Document::new("scene", AssetId::parse("sc_streaming").unwrap());
    let edit = EditWorld::new(registry.clone(), scene).unwrap();
    let session = Session::local(edit, TickConfig::default(), 1, Default::default()).unwrap();
    (registry, session)
}

#[test]
fn arrivals_are_boundary_visible_stably_mapped_and_snapshot_replayable() {
    let (registry, mut session) = setup();
    session.set_hash_sampling(true);
    let build = StateHash::of_bytes(b"streaming-test");
    let before = session.snapshot(build).unwrap();
    let parent = registry
        .prepare_stream_entity(
            "e_parent".into(),
            serde_json::from_value(json!({"name":"Parent","Transform":{"position":[1,2,3]}}))
                .unwrap(),
        )
        .unwrap();
    let child = registry
        .prepare_stream_entity(
            "e_child".into(),
            serde_json::from_value(json!({"parent":"e_parent","Transform":{"position":[0,1,0]}}))
                .unwrap(),
        )
        .unwrap();
    assert!(session.entity("e_parent").is_none());
    let record = session
        .step_with_streaming(|boundary| {
            boundary.spawn(parent)?;
            boundary.spawn(child)
        })
        .unwrap();
    assert_eq!(record.streaming.len(), 2);
    let parent_handle = session.projection().entity("e_parent").unwrap();
    assert_eq!(
        session
            .world()
            .get::<Transform>(parent_handle)
            .unwrap()
            .position,
        [1.0, 2.0, 3.0]
    );
    assert_eq!(
        session.projection().children(Some("e_parent")),
        vec!["e_child"]
    );
    let after = session.snapshot(build).unwrap();
    session.restore(&before, build).unwrap();
    let replay = session
        .step_with_streaming(|boundary| {
            for change in record.streaming {
                boundary.replay(change)?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(record.hash, replay.hash);
    session.restore(&after, build).unwrap();
    session
        .step_with_streaming(|boundary| {
            boundary.despawn("e_child")?;
            boundary.despawn("e_parent")
        })
        .unwrap();
    assert!(session.entity("e_parent").is_none());
    assert!(session.projection().entity("e_child").is_none());
}

#[test]
fn bad_rows_do_not_enter_ecs_and_parent_unload_cannot_cascade() {
    let (registry, mut session) = setup();
    assert!(
        registry
            .prepare_stream_entity(
                "e_bad".into(),
                serde_json::from_value(json!({"Unknown":{"x":1}})).unwrap()
            )
            .is_err()
    );
    let parent = registry
        .prepare_stream_entity("e_parent".into(), Default::default())
        .unwrap();
    let child = registry
        .prepare_stream_entity(
            "e_child".into(),
            serde_json::from_value(json!({"parent":"e_parent"})).unwrap(),
        )
        .unwrap();
    session
        .step_with_streaming(|boundary| {
            boundary.spawn(parent)?;
            boundary.spawn(child)
        })
        .unwrap();
    assert!(
        session
            .step_with_streaming(|boundary| boundary.despawn("e_parent"))
            .is_err()
    );
    assert!(session.entity("e_parent").is_some());
    assert!(session.entity("e_child").is_some());
}
