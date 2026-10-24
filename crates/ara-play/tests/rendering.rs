//! Format-1 snapshots preserve old senders and typed project rendering defaults.
#![allow(clippy::unwrap_used)]
#[test]
fn rendering_defaults_survive_old_snapshots_and_cbor_round_trips() {
    let legacy = serde_json::json!({"format":1,"scene":"sc_scene","documents":[],"types":[],
        "seed":1,"tick_hz":60,"assets":{},"cache":"cache","nav_cache":null});
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
    let mut scene = ara_play::PlayScene::decode(&bytes).unwrap();
    assert_eq!(scene.rendering.quality, ara_play::QualitySetting::Auto);
    scene.rendering.quality = ara_play::QualitySetting::Epic;
    assert_eq!(
        ara_play::PlayScene::decode(&scene.encode().unwrap()).unwrap(),
        scene
    );
}
