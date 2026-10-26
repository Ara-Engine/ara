#![allow(clippy::unwrap_used)]
//! Checked-in scenes saved with the former `PostProcess` component load as a global
//! post-process volume that blends back to the same settings.
use ara_components::post::{PostProcessVolume, PostValues, PostVolume};
use ara_components::render::SceneLook;
use std::path::Path;

#[test]
fn the_outpost_scene_post_process_loads_as_a_global_volume() {
    let schema = ara_runtime::schema(&[]).unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/outpost/levels/outpost.scene");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"PostProcess\""),
        "the sample predates volumes"
    );
    let doc = ara_doc::read(&text, "levels/outpost.scene", &schema, None)
        .doc
        .unwrap();
    let record = doc.record("e_env").unwrap();
    let volume: PostProcessVolume =
        serde_json::from_value(record.get("PostProcessVolume").cloned().unwrap()).unwrap();
    let look = SceneLook {
        volumes: vec![PostVolume {
            volume,
            translation: [0.; 3],
            rotation: [0., 0., 0., 1.],
            scale: [1.; 3],
        }],
        ..Default::default()
    };
    let post = look.post_at([40., 6., -30.]).unwrap();
    let expected = PostValues {
        compensation: -0.5,
        bloom: 0.05,
        ..PostValues::default()
    };
    assert_eq!(post, expected);
}
