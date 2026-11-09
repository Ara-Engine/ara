//! Actual schema, typed command rollback and Undo; no worker or profile is opened.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_cmd::{Batch, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};

fn authority() -> (Registry, Store) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut registry = Registry::builtin(&mut db);
    register(&mut registry, &mut db).unwrap();
    let mut workspace = Workspace::default();
    workspace.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_defaults").unwrap()),
    );
    (registry, Store::new(db, workspace))
}
fn record(store: &Store) -> Value {
    json!(
        store
            .workspace()
            .doc("ip_project")
            .unwrap()
            .record("pi_settings")
            .unwrap()
    )
}
fn apply(registry: &Registry, store: &mut Store, args: Value) {
    registry
        .apply(
            store,
            &Batch::new("main.scene", [("import.project.configure", args)]),
            &Session::human(),
        )
        .unwrap();
}

#[test]
fn typed_type_defaults_preserve_legacy_preset_and_reset_one_type_with_one_undo() {
    let (registry, mut store) = authority();
    apply(&registry, &mut store, json!({"preset":"fast"}));
    apply(
        &registry,
        &mut store,
        json!({"texture":{"usage":"normal","maxDimension":1024},"audio":{"usage":"music","opusBitrate":96000}}),
    );
    let before = record(&store);
    assert_eq!(before["preset"], "fast");
    assert_eq!(before["defaults"]["texture"]["usage"], "normal");
    assert_eq!(before["defaults"]["audio"]["opusBitrate"], 96000);
    let revision = store.revision();
    apply(&registry, &mut store, json!({"resetType":"texture"}));
    assert_eq!(store.revision(), revision + 1);
    let reset = record(&store);
    assert!(reset["defaults"]["texture"].is_null());
    assert_eq!(reset["defaults"]["audio"], before["defaults"]["audio"]);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(record(&store), before);
    store.redo_last(&Author::Human).unwrap();
    assert_eq!(record(&store), reset);
}
#[test]
fn invalid_typed_range_or_incompatible_field_rolls_back_the_whole_policy_batch() {
    let (registry, mut store) = authority();
    apply(&registry, &mut store, json!({"preset":"balanced"}));
    for invalid in [
        json!({"texture":{"maxDimension":0}}),
        json!({"audio":{"opusBitrate":1}}),
        json!({"fbx":{"opacityMaps":{"image":"mask.png"}}}),
        json!({"texture":{"invented":true}}),
        json!({"resetType":"audio","audio":{}}),
    ] {
        let before = record(&store);
        let revision = store.revision();
        let batch = Batch::new(
            "main.scene",
            [
                ("import.project.configure", json!({"preset":"highQuality"})),
                ("import.project.configure", invalid),
            ],
        );
        assert!(
            registry
                .apply(&mut store, &batch, &Session::human())
                .is_err()
        );
        assert_eq!(record(&store), before);
        assert_eq!(store.revision(), revision);
    }
}
#[test]
fn moved_types_are_the_same_public_settings_and_audio_defaults_remain_exact() {
    use std::any::TypeId;
    assert_eq!(
        TypeId::of::<crate::TextureSettings>(),
        TypeId::of::<ara_asset::TextureSettings>()
    );
    assert_eq!(
        TypeId::of::<crate::MeshSettings>(),
        TypeId::of::<ara_asset::MeshSettings>()
    );
    assert_eq!(
        TypeId::of::<crate::audio::Settings>(),
        TypeId::of::<ara_asset::AudioImportSettings>()
    );
    let shared = crate::audio::Settings::default();
    let runtime = ara_audio::importer::CookSettings::default();
    assert_eq!(shared.opus_bitrate, runtime.opus_bitrate);
    assert_eq!(shared.decoded_budget, runtime.decoded_budget as u64);
    assert!(matches!(shared.usage, crate::audio::Usage::ShortEffect));
    assert!(matches!(shared.codec, crate::audio::EffectCodec::Pcm));
}
