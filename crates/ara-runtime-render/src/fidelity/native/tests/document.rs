use super::*;

#[test]
fn editor_document_uses_runtime_surface_effect_and_light_extraction() {
    let record = serde_json::from_value(serde_json::json!({
        "name":"Editor actor",
        "MeshRenderer":{"emissive":[0.2,0.3,0.4],"color":[0.3,0.5,0.7,1]},
        "Environment":{"intensity":42},
        "VisualEffect":{"effect":"campfire"},
        "Light":{"intensity":120},
        "Camera":{"fov":62}
    }))
    .unwrap();
    let entities = BTreeMap::from([(
        "e_actor".into(),
        ara_doc::prefab::ResolvedEntity {
            record,
            parent: None,
            order: None,
            source: (
                ara_doc::AssetId::parse("sc_editor").unwrap(),
                ara_doc::RecordId::parse("e_actor").unwrap(),
            ),
        },
    )]);
    let mut matrix = glam30::Mat4::from_translation(glam30::Vec3::new(3.0, 4.0, 5.0));
    matrix.y_axis.x = 0.4;
    let transforms = BTreeMap::from([("e_actor".into(), matrix)]);
    let mut library = ara_runtime::animation::Library::default();
    let frame = document_frame(
        &entities,
        &transforms,
        &ara_doc::Workspace::default(),
        &mut library,
        &ara_runtime::assets::MemoryAssets::default(),
        None,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(frame.draws.len(), 1);
    assert!(
        frame.draws[0]
            .world
            .abs_diff_eq(glam::Mat4::from_cols_array(&matrix.to_cols_array()), 1e-5)
    );
    assert_eq!(frame.draws[0].color, [0.3, 0.5, 0.7, 1.0]);
    assert_eq!(frame.draws[0].emissive, [0.2, 0.3, 0.4]);
    assert_eq!(frame.effects[0].position, [3.0, 4.0, 5.0]);
    assert_eq!(frame.effects[0].effect, "campfire");
    assert_eq!(frame.lights[0].lumens, 120.0);
    assert_eq!(frame.lights[0].position, [3.0, 4.0, 5.0]);
    assert_eq!(frame.environment.intensity, 42.0);
    assert_eq!(frame.camera.unwrap().fov, 62.0);

    let hidden = std::collections::BTreeSet::from(["e_actor".to_owned()]);
    let frame = document_frame(
        &entities,
        &transforms,
        &ara_doc::Workspace::default(),
        &mut library,
        &ara_runtime::assets::MemoryAssets::default(),
        None,
        &hidden,
    )
    .unwrap();
    assert!(frame.draws.is_empty());
    assert!(frame.lights.is_empty());
    assert!(frame.effects.is_empty());
    assert!(frame.camera.is_none());
}
