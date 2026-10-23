#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{
    document::{self, AnimAsset, AnimationCommand},
    graph::ParameterKind,
};
use ara_doc::{AssetId, AssetIndex, IdGen, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::SchemaDb;

#[test]
fn schema_canonical_documents_typed_ops_and_undo_share_the_kernel() {
    let mut db = SchemaDb::new();
    document::register(&mut db).unwrap();
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    let text = r#"{"records":{},"id":"ac_test0001","$schema":"ara://schema/animation/1"}"#;
    let doc = document::read(text, "rig.animation", &db).unwrap();
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("rig.animation", doc);
    let mut store = Store::with_ids(db, ws, IdGen::seeded(42));
    let id = AssetId::parse("ac_test0001").unwrap();
    let record = RecordId::parse("an_rig").unwrap();
    let s = common::skeleton(2);
    let asset = AnimAsset {
        name: "Fox".into(),
        skeleton: Some(s.data().clone()),
        clip: Some(common::clip(&s, 1.0)),
        controller: Some(common::controller_data()),
        rig: Some(ara_anim::retarget::RigMap {
            bones: vec![ara_anim::retarget::RigBone {
                role: ara_anim::retarget::BoneRole::Root,
                joint: "joint_0".into(),
            }],
        }),
        ik: Some(document::IkSetup {
            pelvis: "joint_0".into(),
            chains: vec![ara_anim::ik::ChainData {
                joints: vec!["joint_0".into(), "joint_1".into()],
                ..Default::default()
            }],
            ..Default::default()
        }),
        motion_database: Some(ara_anim::matching::MatchConfig {
            bones: vec!["joint_1".into()],
            ..Default::default()
        }),
        ragdoll: Some(ara_anim::physical::RagdollProfile {
            bones: vec![ara_anim::physical::RagdollBone {
                joint: "joint_0".into(),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    AnimationCommand::Create(asset.clone())
        .apply(
            &mut store,
            TxnMeta::by(Author::Human),
            id.clone(),
            record.clone(),
        )
        .unwrap();
    let written = document::write(store.workspace().doc(id.as_str()).unwrap(), store.schema());
    let read = document::read(&written, "rig.animation", store.schema()).unwrap();
    assert_eq!(written, document::write(&read, store.schema()));
    assert_eq!(document::asset(&read, &record).unwrap(), asset);
    // Omitted schema defaults are reconstructed by serde defaults on every type.
    assert_eq!(
        document::asset(&read, &record)
            .unwrap()
            .controller
            .unwrap()
            .parameters["go"]
            .kind,
        ParameterKind::Trigger
    );
    let transaction = AnimationCommand::SetField {
        path: "controller.nodes[locomotion].weight".parse().unwrap(),
        value: Some(serde_json::json!(0.75)),
    }
    .apply(
        &mut store,
        TxnMeta::by(Author::Agent("animation".into())),
        id.clone(),
        record,
    )
    .unwrap()
    .txn()
    .unwrap()
    .id
    .clone();
    store.undo(&transaction, Author::Human).unwrap();
    assert_eq!(
        written,
        document::write(store.workspace().doc(id.as_str()).unwrap(), store.schema())
    );
    assert!(document::read(r#"{"id":1,"id":2}"#, "bad.animation", store.schema()).is_err());
    for command in [
        AnimationCommand::SetRig(asset.rig.unwrap()),
        AnimationCommand::SetIk(asset.ik.unwrap()),
        AnimationCommand::SetMotionDatabase(asset.motion_database.unwrap()),
        AnimationCommand::SetRagdoll(asset.ragdoll.unwrap()),
    ] {
        command
            .apply(
                &mut store,
                TxnMeta::by(Author::Human),
                id.clone(),
                RecordId::parse("an_rig").unwrap(),
            )
            .unwrap();
    }
}
