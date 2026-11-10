//! Consuming a decoded snapshot must retain the complete authoring/runtime contract.
#[path = "support/scene.rs"]
mod fixture;
use ara_play::PlayScene;
use ara_sim::{PlayControl, Session, Snapshot, StateHash, TickConfig};
use serde_json::json;

#[test]
fn transfer_moves_record_storage_and_retains_metadata_and_retry_bytes() {
    let schema = fixture::schema();
    let source = fixture::source(65);
    let scene = fixture::snapshot(&source);
    let retry = scene.encode().expect("CBOR");
    let mut received = PlayScene::decode(&retry).expect("decode");
    let title = received
        .documents
        .iter()
        .find(|d| d.kind == "scene")
        .expect("scene")
        .records["e_00000000"]["Stats"]["title"]
        .as_str()
        .expect("title")
        .as_ptr();
    let mut metadata = received.clone();
    metadata.documents.clear();
    let (workspace, id) = received.take_workspace().expect("transfer");
    assert_eq!(received, metadata, "all other snapshot fields survive");
    assert_eq!(id.as_str(), "sc_bench");
    assert_eq!(
        workspace
            .doc("sc_bench")
            .expect("scene")
            .record("e_00000000")
            .expect("record")["Stats"]["title"]
            .as_str()
            .expect("title")
            .as_ptr(),
        title,
        "JSON allocation moved"
    );
    for (path, doc) in source.docs() {
        assert_eq!(workspace.path_of(doc.id.as_str()), Some(path));
        assert_eq!(
            ara_doc::write(doc, &schema),
            ara_doc::write(
                workspace
                    .doc(doc.id.as_str())
                    .expect("source document retained"),
                &schema
            )
        );
    }
    assert_eq!(
        workspace.index().path_of("as_beef"),
        Some("audio/effect.wav")
    );
    assert_eq!(PlayScene::decode(&retry).expect("retry snapshot"), scene);
    assert_eq!(
        PlayScene::documents(&source),
        scene.documents,
        "source owner untouched"
    );
}

#[test]
fn transfer_matches_borrowed_errors_and_document_replacement_order() {
    let scene = fixture::snapshot(&fixture::source(3));
    let mut invalid_doc = scene.clone();
    invalid_doc.documents[0].id = "bad!".into();
    invalid_doc.scene = "also bad!".into();
    let mut invalid_record = scene.clone();
    invalid_record.documents[0].records.insert(
        "bad!".into(),
        json!({}).as_object().expect("object").clone(),
    );
    invalid_record.scene = "also bad!".into();
    let mut invalid_scene = scene.clone();
    invalid_scene.scene = "bad!".into();
    for mut candidate in [invalid_doc, invalid_record, invalid_scene] {
        let expected = fixture::borrowed_workspace(&candidate).expect_err("invalid snapshot");
        assert_eq!(
            candidate.take_workspace().expect_err("same error"),
            expected
        );
        assert!(
            candidate.documents.is_empty(),
            "one-shot documents consumed on error"
        );
    }
    let mut replacement = scene;
    let mut last = replacement.documents[0].clone();
    last.records.clear();
    replacement.documents.push(last);
    let (expected, id) = fixture::borrowed_workspace(&replacement).expect("borrowed");
    let (actual, actual_id) = replacement.take_workspace().expect("owned");
    assert_eq!(actual_id, id);
    assert_eq!(
        PlayScene::documents(&actual),
        PlayScene::documents(&expected)
    );
}

#[test]
fn projected_dependencies_live_changes_portable_restore_and_replay_agree() {
    let schema = fixture::schema();
    let scene = fixture::snapshot(&fixture::source(65));
    let mut received = PlayScene::decode(&scene.encode().expect("CBOR")).expect("decode");
    let (before, id) = fixture::borrowed_workspace(&received).expect("borrowed");
    let (after, after_id) = received.take_workspace().expect("owned");
    let mut a = fixture::project(&schema, before, id);
    let mut b = fixture::project(&schema, after, after_id);
    let identities = a
        .world()
        .iter_entities()
        .filter_map(|e| {
            let id = e.get::<ara_sim::Identity>()?.0.clone();
            Some((
                id.clone(),
                a.entity(&id).expect("mapped"),
                b.entity(&id).expect("mapped"),
            ))
        })
        .collect::<Vec<_>>();
    assert!(
        a.entity("e_00000010/e_0002").is_some(),
        "prefab child resolved"
    );
    fixture::change(&mut a);
    fixture::change(&mut b);
    for (id, before_a, before_b) in identities {
        assert_eq!(a.entity(&id), Some(before_a), "borrowed identity reuse");
        assert_eq!(b.entity(&id), Some(before_b), "owned identity reuse");
    }
    let control = |edit| {
        PlayControl::new(
            Session::local(
                edit,
                TickConfig::default(),
                901,
                ara_sim::input::ActionMap::default(),
            )
            .expect("session"),
            StateHash::of_bytes(b"transfer-test"),
        )
    };
    let mut a = control(a);
    let mut b = control(b);
    assert_eq!(
        a.session().state_hash().expect("hash"),
        b.session().state_hash().expect("hash")
    );
    assert_eq!(
        a.snapshot()
            .expect("snapshot")
            .to_bytes()
            .expect("portable"),
        b.snapshot()
            .expect("snapshot")
            .to_bytes()
            .expect("portable")
    );
    let snapshot = a.snapshot().expect("snapshot");
    let portable = Snapshot::from_bytes(&snapshot.to_bytes().expect("encode")).expect("decode");
    a.start();
    a.record().expect("record");
    a.step(3, &[]).expect("steps");
    let expected = a.session().state_hash().expect("hash");
    let replay = a.finish_recording().expect("recording");
    let replay = ara_sim::Replay::from_bytes(&replay.to_bytes().expect("encode")).expect("decode");
    b.start();
    assert_eq!(b.replay(&replay).expect("replay"), expected);
    a.restore(&portable).expect("portable restore");
    assert_eq!(
        a.snapshot()
            .expect("restored snapshot")
            .to_bytes()
            .expect("portable"),
        snapshot.to_bytes().expect("portable")
    );
}
