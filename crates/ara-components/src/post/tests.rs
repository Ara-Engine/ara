use super::*;
use crate::render::SceneLook;
use ara_schema::SchemaDb;

fn global(priority: f32, weight: f32, overrides: Vec<PostOverride>) -> PostVolume {
    PostVolume {
        volume: PostProcessVolume {
            priority,
            weight,
            overrides,
            ..Default::default()
        },
        translation: [0.; 3],
        rotation: [0., 0., 0., 1.],
        scale: [1.; 3],
    }
}

fn bloom(value: f32) -> PostOverride {
    PostOverride {
        module: PostModule::Bloom,
        bloom: Some(value),
        ..Default::default()
    }
}

#[test]
fn a_saved_post_process_loads_as_a_global_volume_that_renders_as_before() {
    let mut db = SchemaDb::new();
    crate::register(&mut db);
    let (ty, via_alias) = db.lookup("PostProcess").expect("aliased");
    assert!(via_alias && ty.name == "PostProcessVolume");
    let mut value = serde_json::json!({
        "exposure": "manual", "ev100": 12, "toneMapping": "neutral", "bloom": 0.06, "saturation": 1.2
    });
    let mut notes = Vec::new();
    assert!(db.normalize_component(ty, 1, &mut value, "PostProcess", &mut notes));
    let volume: PostProcessVolume = serde_json::from_value(value).expect("upgraded volume");
    assert_eq!(volume.shape, VolumeShape::Global);
    let look = SceneLook {
        volumes: vec![PostVolume {
            volume,
            ..global(0., 1., Vec::new())
        }],
        ..Default::default()
    };
    let post = look.post_at([500., 3., -200.]).expect("volume");
    let expected = PostValues {
        exposure: ExposureMode::Manual,
        ev100: 12.,
        tone_mapping: ToneMapping::Neutral,
        bloom: 0.06,
        saturation: 1.2,
        ..PostValues::default()
    };
    assert_eq!(post, expected);
}

#[test]
fn a_volume_written_without_a_versions_entry_is_not_migrated_again() {
    let mut db = SchemaDb::new();
    crate::register(&mut db);
    let ty = db.get("PostProcessVolume").expect("registered");
    let mut value = serde_json::json!({
        "overrides": [{"module": "vignette", "vignette": 0.3}]
    });
    let mut notes = Vec::new();
    // Stored version 1 (no `versions` table), as hand-written scene JSON reads.
    assert!(db.normalize_component(ty, 1, &mut value, "PostProcessVolume", &mut notes));
    let volume: PostProcessVolume = serde_json::from_value(value).expect("volume");
    assert_eq!(volume.overrides.len(), 1);
    assert_eq!(volume.overrides[0].vignette, Some(0.3));
}

#[test]
fn volumes_blend_in_priority_order_by_their_weight() {
    // Applied 0.25 then 0.75 regardless of list order: the higher priority lands last.
    let look = SceneLook {
        volumes: vec![
            global(5., 0.5, vec![bloom(0.2)]),
            global(-1., 1., vec![bloom(0.1)]),
        ],
        ..Default::default()
    };
    let post = look.post_at([0.; 3]).expect("volumes");
    assert!((post.bloom - (0.1 + (0.2 - 0.1) * 0.5)).abs() < 1e-6);
    assert!(SceneLook::default().post_at([0.; 3]).is_none());
}

#[test]
fn local_volumes_fade_over_their_blend_distance() {
    let mut box_volume = global(0., 1., vec![bloom(0.2)]);
    box_volume.volume.shape = VolumeShape::Box;
    box_volume.volume.blend_distance = 4.;
    box_volume.translation = [10., 0., 0.];
    box_volume.scale = [2., 2., 2.];
    // Inside, half-way through the blend band, and beyond it.
    assert_eq!(box_volume.weight_at([10.5, 0.5, 0.]), 1.);
    assert!((box_volume.weight_at([13., 0., 0.]) - 0.5).abs() < 1e-5);
    assert_eq!(box_volume.weight_at([20., 0., 0.]), 0.);
    let mut sphere = box_volume.clone();
    sphere.volume.shape = VolumeShape::Sphere;
    sphere.volume.blend_distance = 0.;
    assert_eq!(sphere.weight_at([10.9, 0., 0.]), 1.);
    assert_eq!(sphere.weight_at([10.9, 0.9, 0.]), 0.);
    sphere.scale = [0.; 3];
    assert_eq!(sphere.weight_at([10., 0., 0.]), 0.);
    let look = SceneLook {
        volumes: vec![box_volume],
        ..Default::default()
    };
    let half = look.post_at([13., 0., 0.]).expect("volume");
    assert!((half.bloom - 0.12).abs() < 1e-6);
}

#[test]
fn only_the_entry_module_applies_and_preset_values_compose() {
    // A stale value in another group's field (after switching `module`) is inert.
    let stale = PostOverride {
        module: PostModule::Vignette,
        vignette: Some(0.5),
        bloom: Some(0.25),
        ..Default::default()
    };
    let radius = |value| PostOverride {
        module: PostModule::AmbientOcclusion,
        ao_radius: Some(value),
        ..Default::default()
    };
    let look = SceneLook {
        volumes: vec![
            global(0., 1., vec![stale]),
            global(1., 0.5, vec![radius(3.)]),
            global(2., 0.5, vec![radius(1.)]),
        ],
        ..Default::default()
    };
    let post = look.post_at([0.; 3]).expect("volumes");
    assert_eq!(
        (post.vignette, post.bloom),
        (0.5, PostValues::default().bloom)
    );
    // Preset 2: lerp(lerp(2, 3, 0.5), 1, 0.5) = 1.75.
    assert!((post.ao_radius.over(2.) - 1.75).abs() < 1e-6);
    assert_eq!(PresetBlend::default().over(2.), 2.);
}
