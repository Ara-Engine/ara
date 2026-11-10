//! Typed policy precedence and action identity, with no worker or filesystem I/O.
use ara_asset::*;
use serde_json::{Value, json};

// Resolve the real texture policy, then hash it with a synthetic importer identity.
fn input(policy: &ProjectImportSettings, source: Value, preset: Option<CookPreset>) -> ImportInput {
    let (settings, preset) = policy
        .resolved_settings("ara.texture", preset, &source)
        .expect("resolved input");
    ImportInput {
        preset,
        thread_budget: 2,
        source_path: "image.png".into(),
        source: vec![1, 2, 3],
        importer: "fixture.texture".into(),
        version: "fixture".into(),
        settings,
        target: TargetPlatform::Windows,
        dependencies: Default::default(),
        asset_dependencies: Default::default(),
    }
}
#[test]
fn typed_defaults_round_trip_the_existing_importer_schemas() {
    let policy = ProjectImportSettings {
        defaults: ImporterDefaults {
            texture: Some(TextureSettings {
                usage: TextureUsage::Normal,
                ..Default::default()
            }),
            gltf: Some(MeshSettings {
                lod_count: 2,
                ..Default::default()
            }),
            audio: Some(AudioImportSettings {
                usage: AudioImportUse::Music,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut db = ara_schema::SchemaDb::new();
    register_schema(&mut db);
    let text = policy.canonical_text().expect("canonical typed policy");
    let read = ara_doc::read(&text, PROJECT_IMPORT_SETTINGS_FILE, &db, None);
    assert!(read.diags.iter().all(|d| !d.is_error()), "{:?}", read.diags);
    let doc = read.doc.expect("policy document");
    let actual: ProjectImportSettings =
        serde_json::from_value(json!(doc.record("pi_settings").expect("record")))
            .expect("typed policy");
    assert_eq!(actual, policy);
    assert!(
        serde_json::from_value::<ProjectImportSettings>(
            json!({"defaults":{"texture":{"invented":true}}})
        )
        .is_err()
    );
}
#[test]
fn explicit_source_fields_and_quality_win_without_crossing_importer_types() {
    let policy = ProjectImportSettings {
        defaults: ImporterDefaults {
            texture: Some(TextureSettings {
                preset: Some(CookPreset::HighQuality),
                usage: TextureUsage::Normal,
                max_dimension: 1024,
                ..Default::default()
            }),
            gltf: Some(MeshSettings {
                lod_count: 2,
                ..Default::default()
            }),
            audio: Some(AudioImportSettings {
                opus_bitrate: 96000,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let cooked = input(
        &policy,
        json!({"maxDimension":4096,"mipmaps":false}),
        Some(CookPreset::Fast),
    );
    assert_eq!(cooked.settings["usage"], "normal");
    assert_eq!(cooked.settings["maxDimension"], 4096);
    assert_eq!(cooked.settings["mipmaps"], false);
    assert_eq!(cooked.settings["preset"], "fast");
    assert_eq!(cooked.preset, CookPreset::Fast);
    assert_eq!(
        input(
            &policy,
            json!({"preset":"balanced"}),
            Some(CookPreset::Fast)
        )
        .preset,
        CookPreset::Balanced
    );
    assert_eq!(
        policy
            .effective_settings("ara.gltf", &json!({}))
            .expect("mesh")["lodCount"],
        2
    );
    assert_eq!(
        policy
            .effective_settings("ara.audio", &json!({}))
            .expect("audio")["opusBitrate"],
        96000
    );
    assert_eq!(
        policy
            .effective_settings("ara.fbx", &json!({}))
            .expect("FBX remains unconfigured"),
        json!({})
    );
}
#[test]
fn defaults_change_action_hash_without_sidecar_edits_and_explicit_overrides_stay_stable() {
    let original = ProjectImportSettings::default();
    for raw in [json!({}), json!({"preset":null,"mipmaps":false})] {
        let resolved = input(&original, raw.clone(), Some(CookPreset::Fast));
        let mut legacy = resolved.clone();
        legacy.settings = raw;
        assert_eq!(
            resolved.settings, legacy.settings,
            "absent/null preset keys retain the historical no-policy input"
        );
        assert_eq!(
            resolved.key().expect("resolved key"),
            legacy.key().expect("legacy no-policy key")
        );
    }
    let mut changed = original.clone();
    changed.defaults.texture = Some(TextureSettings {
        usage: TextureUsage::Normal,
        ..Default::default()
    });
    let source = json!({});
    assert_ne!(
        input(&original, source.clone(), None).key().expect("key"),
        input(&changed, source, None).key().expect("new key")
    );
    let explicit =
        serde_json::to_value(TextureSettings::default()).expect("all explicit source keys");
    assert_eq!(
        input(&original, explicit.clone(), Some(CookPreset::Fast))
            .key()
            .expect("key"),
        input(&changed, explicit, Some(CookPreset::Fast))
            .key()
            .expect("overridden key")
    );
    changed.defaults.texture = None;
    assert_eq!(
        input(&original, json!({}), None)
            .key()
            .expect("original key"),
        input(&changed, json!({}), None)
            .key()
            .expect("reset restores key")
    );
}
