#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::input::InputEvent;
mod fixture;
use fixture::{Clip, Fixture};

#[test]
fn real_schema_creations_are_one_content_transaction_with_local_size_and_global_fog() {
    let mut f = Fixture::new();
    for kind in [
        Create::Global,
        Create::Box,
        Create::Sphere,
        Create::HeightFog,
    ] {
        let count = f.editor.host.store().history().len();
        let camera = f.editor.state.camera;
        let entity = f.create(kind);
        assert_eq!(f.editor.host.store().history().len(), count + 1);
        assert_eq!(f.editor.state.camera, camera);
        if matches!(kind, Create::HeightFog) {
            let fog: ara_components::render::HeightFog =
                serde_json::from_value(f.component(&entity, "HeightFog")).unwrap();
            assert_eq!(fog, ara_components::render::HeightFog::default());
        } else {
            let volume: PostProcessVolume =
                serde_json::from_value(f.component(&entity, "PostProcessVolume")).unwrap();
            assert_eq!(
                volume.shape,
                match kind {
                    Create::Global => VolumeShape::Global,
                    Create::Box => VolumeShape::Box,
                    _ => VolumeShape::Sphere,
                }
            );
            assert!(volume.overrides.is_empty());
            if !matches!(kind, Create::Global) {
                assert_eq!(volume.blend_distance, 2.);
                let transform: ara_components::Transform =
                    serde_json::from_value(f.component(&entity, "Transform")).unwrap();
                assert!(transform.scale.iter().all(|size| *size >= 5.));
            }
        }
        f.editor.action("history.undo", json!({})).unwrap();
        assert!(
            !crate::world::resolve(f.editor.host.store(), &f.editor.state.doc)
                .unwrap()
                .contains_key(&entity)
        );
        f.editor.action("history.redo", json!({})).unwrap();
        assert!(
            crate::world::resolve(f.editor.host.store(), &f.editor.state.doc)
                .unwrap()
                .contains_key(&entity)
        );
    }
}

#[test]
fn presets_append_latest_per_object_groups_in_one_undo_and_reject_mixed_nonvolumes_atomically() {
    let mut f = Fixture::new();
    let a = f.create(Create::Global);
    let b = f.create(Create::Box);
    f.editor.apply(vec![json!({"cmd":"property.set","args":{"entity":a,"path":"PostProcessVolume.overrides","value":[{"module":"bloom","bloom":0.08} ]}}),json!({"cmd":"property.set","args":{"entity":b,"path":"PostProcessVolume.overrides","value":[{"module":"vignette","vignette":0.3}]}})],"Different per-volume groups",None).unwrap();
    let before = [
        f.component(&a, "PostProcessVolume"),
        f.component(&b, "PostProcessVolume"),
    ];
    f.editor
        .action("editor.select", json!({"entities":[a,b]}))
        .unwrap();
    let count = f.editor.host.store().history().len();
    f.shell
        .lighting_preset(&mut f.editor, Preset::Interior)
        .unwrap();
    assert_eq!(f.editor.host.store().history().len(), count + 1);
    for (index, entity) in [&a, &b].into_iter().enumerate() {
        let after = f.component(entity, "PostProcessVolume");
        let overrides = after["overrides"].as_array().unwrap();
        assert_eq!(overrides.len(), 4);
        assert_eq!(overrides[0], before[index]["overrides"][0]);
        assert_eq!(overrides[1]["module"], "exposure");
        assert_eq!(overrides[1]["ev100"].as_f64(), Some(7.));
        assert_eq!(overrides[2]["whiteBalance"].as_f64(), Some(4500.));
    }
    f.editor.action("history.undo", json!({})).unwrap();
    assert_eq!(f.component(&a, "PostProcessVolume"), before[0]);
    assert_eq!(f.component(&b, "PostProcessVolume"), before[1]);
    f.editor.action("history.redo", json!({})).unwrap();
    let fog = f.create(Create::HeightFog);
    f.editor
        .action("editor.select", json!({"entities":[a,fog]}))
        .unwrap();
    let revision = f.editor.host.store().revision();
    let before = f.component(&a, "PostProcessVolume");
    assert!(
        f.shell
            .lighting_preset(&mut f.editor, Preset::ManualExposure)
            .is_err()
    );
    assert_eq!(f.editor.host.store().revision(), revision);
    assert_eq!(f.component(&a, "PostProcessVolume"), before);
}

#[test]
fn local_camera_influence_and_selected_bounds_follow_parent_world_pose_and_hide_state() {
    let mut f = Fixture::new();
    f.editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":"e_lparent","components":{"Transform":{"position":[10,0,0],"rotation":[0,90,0],"scale":[2,1,1]}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_lbox","parent":"e_lparent","components":{"Transform":{"scale":[4,4,4]},"PostProcessVolume":{"shape":"box","weight":0.8,"blendDistance":2}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_lglobal","components":{"PostProcessVolume":{}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_lzero","components":{"Transform":{"scale":[0,1,1]},"PostProcessVolume":{"shape":"sphere"}}}}),
    ],"Volume bounds fixture",None).unwrap();
    let resolved = crate::world::resolve(f.editor.host.store(), &f.editor.state.doc).unwrap();
    let volume = model::volume(&resolved, "e_lbox").unwrap();
    assert!((volume.weight_at([10., 0., 0.]) - 0.8).abs() < 0.0001);
    assert!((volume.weight_at([13., 0., 0.]) - 0.4).abs() < 0.0001);
    assert_eq!(volume.weight_at([15., 0., 0.]), 0.);
    let selected = vec!["e_lbox".into(), "e_lglobal".into(), "e_lzero".into()];
    let bounds = bounds::selected(&resolved, &selected, &Default::default());
    assert_eq!(bounds.len(), 1);
    assert_eq!(bounds[0].entity, "e_lbox");
    assert_eq!(bounds[0].blend_distance, 2.);
    let edges: Vec<_> = bounds[0].segments().collect();
    assert_eq!(edges.len(), 12);
    assert!(edges.iter().flatten().all(|point| point.is_finite()));
    assert!(
        edges
            .iter()
            .flatten()
            .all(|point| point.x >= 7.999 && point.x <= 12.001)
    );
    assert!(
        bounds::selected(
            &resolved,
            &selected,
            &std::collections::BTreeSet::from(["e_lbox".into()])
        )
        .is_empty()
    );
    let overflowing = glam::Mat4::from_scale(Vec3::new(1e30, 1e-10, 1e-10));
    assert!(
        bounds::from_entity("e_lbox", &resolved["e_lbox"].record, overflowing).is_none(),
        "finite authored values must not explode during world-pose decomposition"
    );
    let sheared = crate::world::world_transform(&resolved, "e_lbox").unwrap()
        * glam::Mat4::from_rotation_z(0.7);
    let bound = bounds::from_entity("e_lbox", &resolved["e_lbox"].record, sheared).unwrap();
    let actual = ara_runtime::math::Pose::from_matrix(glam34::Mat4::from_cols_array(
        &sheared.to_cols_array(),
    ))
    .matrix()
    .to_cols_array();
    assert!(
        bound
            .world
            .to_cols_array()
            .into_iter()
            .zip(actual)
            .all(|(a, b)| (a - b).abs() < 0.0001),
        "cages discard shear and normalize rotation exactly as runtime Pose extraction"
    );
    assert!(bound.segments().flatten().all(|point| point.is_finite()));
}

#[test]
fn lighting_inspector_uses_native_select_and_scrub_one_undo_without_duplicate_mounted_ids() {
    let mut f = Fixture::new();
    f.editor
        .action(
            "editor.layout.move",
            json!({"panel":"lighting","slot":"view"}),
        )
        .unwrap();
    let entity = f.create(Create::Box);
    f.editor.apply(vec![json!({"cmd":"property.set","args":{"entity":entity,"path":"PostProcessVolume.weight","value":0.4}})],"Initial volume weight",None).unwrap();
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    assert!(ara_ui_test::audit_attached_ids(&f.shell.ui).is_empty());
    let shape = f.shell.shown_node("enum-PostProcessVolume-shape").unwrap();
    assert_eq!(f.shell.ui.nodes[shape].element.tag, "dropdown");
    let weight = f.shell.shown_node("edit-PostProcessVolume-weight").unwrap();
    let rect = f.shell.ui.nodes[weight].rect;
    let clip = f.shell.ui.nodes[weight].clip;
    assert!(rect.intersection(clip).h >= rect.h - 0.5);
    let point = [rect.x + rect.w - 5., rect.y + rect.h * 0.5];
    let end = [point[0] + 24., point[1]];
    let revision = f.editor.host.store().revision();
    f.shell
        .event(InputEvent::PointerDown(point), &mut Clip)
        .unwrap();
    f.shell
        .event(InputEvent::PointerMove(end), &mut Clip)
        .unwrap();
    f.shell.dispatch(&mut f.editor).unwrap();
    assert_eq!(
        f.editor.host.store().revision(),
        revision,
        "scrub preview is not a document mutation"
    );
    f.shell
        .event(InputEvent::PointerUp(end), &mut Clip)
        .unwrap();
    f.shell.dispatch(&mut f.editor).unwrap();
    assert_eq!(f.editor.host.store().revision(), revision + 1);
    let value = f.component(&entity, "PostProcessVolume")["weight"]
        .as_f64()
        .unwrap();
    assert!(value > 0.4 && value <= 1.);
    f.editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        f.component(&entity, "PostProcessVolume")["weight"].as_f64(),
        Some(0.4)
    );
    f.editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    f.shell.ui.resize([320., 450.]).unwrap();
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    let shape = f.shell.shown_node("enum-PostProcessVolume-shape").unwrap();
    let node = &f.shell.ui.nodes[shape];
    assert!(node.rect.w > 0. && node.rect.x >= 0. && node.rect.x + node.rect.w <= 320.5);
    assert!(ara_ui_test::audit_attached_ids(&f.shell.ui).is_empty());
}

#[test]
fn inventory_pages_all_matches_and_explicit_inspect_reveals_a_filtered_late_entity() {
    let mut f = Fixture::new();
    let calls=(0..130).map(|index|json!({"cmd":"entity.create","args":{"id":format!("e_lpage{index:03}"),"name":format!("Lighting row {index}"),"components":{"Light":{"kind":"point"}}}})).collect();
    f.editor
        .apply(calls, "Lighting inventory pagination fixture", None)
        .unwrap();
    f.shell.open_lighting(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    let mounted = |shell: &Shell| {
        shell
            .ui
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, node)| {
                shell.ui.shown(*index)
                    && node.element.tag == "button"
                    && node.element.id.starts_with("lighting-entry-")
            })
            .count()
    };
    assert_eq!(mounted(&f.shell), 64);
    let revision = f.editor.host.store().revision();
    let camera = f.editor.state.camera;
    f.shell.page_lighting(&mut f.editor, true).unwrap();
    f.shell.frame().unwrap();
    assert_eq!(mounted(&f.shell), 64);
    f.shell.page_lighting(&mut f.editor, true).unwrap();
    f.shell.frame().unwrap();
    assert_eq!(mounted(&f.shell), 2);
    f.editor
        .action(
            "editor.search",
            json!({"panel":"lighting","query":"No matching volume"}),
        )
        .unwrap();
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    assert_eq!(mounted(&f.shell), 0);
    f.shell
        .inspect_lighting(&mut f.editor, &Target::Entity("e_lpage129".into()))
        .unwrap();
    f.shell.frame().unwrap();
    assert_eq!(f.editor.state.selection, ["e_lpage129"]);
    assert!(
        f.editor
            .state
            .searches
            .get("lighting")
            .is_some_and(String::is_empty)
    );
    assert_eq!(f.shell.lighting.start, 128);
    assert!(
        f.shell
            .shown_node(&format!(
                "lighting-entry-{:016x}",
                ara_ui::source::stable_id("e_lpage129")
            ))
            .is_some()
    );
    assert_eq!(f.editor.host.store().revision(), revision);
    assert_eq!(f.editor.state.camera, camera);
}
