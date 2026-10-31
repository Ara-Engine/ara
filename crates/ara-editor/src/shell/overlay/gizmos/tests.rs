//! Gizmo classification, settings, shape geometry and icon picking without a GPU.
use super::*;
use crate::state::CameraProjection;
use ara_doc::{AssetId, Record, RecordId, prefab::ResolvedEntity};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

const RECT: Box2 = Box2 {
    x: 0.,
    y: 0.,
    w: 800.,
    h: 600.,
};

fn record(components: serde_json::Value) -> Record {
    let mut record = Record::new();
    for (key, value) in components.as_object().expect("components object") {
        record.insert(key.clone(), value.clone());
    }
    record
}
fn view(entities: &[(&str, Record)]) -> DocumentView {
    let entities = entities
        .iter()
        .map(|(id, record)| {
            (
                (*id).to_owned(),
                ResolvedEntity {
                    record: record.clone(),
                    parent: None,
                    order: None,
                    source: (
                        AssetId::parse("sc_gizmotest1").expect("fixture scene ID"),
                        RecordId::parse(id).expect("fixture entity ID"),
                    ),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    DocumentView::with_imports(Arc::new(entities), Arc::default())
}
fn state(selection: &[&str]) -> EditorState {
    EditorState {
        camera: Camera {
            target: Vec3::ZERO,
            yaw: 0.4,
            pitch: -0.4,
            distance: 30.,
            projection: CameraProjection::Perspective,
            ..Camera::default()
        },
        selection: selection.iter().map(|s| (*s).to_owned()).collect(),
        ..EditorState::default()
    }
}
fn color(token: &str, alpha: f32) -> ara_ui::Result<[f32; 4]> {
    Ok([token.len() as f32 / 32., 0.5, 0.5, alpha])
}
fn lines(overlays: &[Overlay]) -> usize {
    overlays
        .iter()
        .filter(|o| matches!(o, Overlay::Line { from, to, .. } if from != to))
        .count()
}
fn draw(gizmos: &mut Gizmos, view: &DocumentView, state: &EditorState) -> Vec<Overlay> {
    let mut overlays = Vec::new();
    gizmos
        .draw(
            &mut overlays,
            view,
            state,
            RECT,
            1,
            GizmoState::default(),
            &color,
        )
        .expect("gizmos draw");
    overlays
}

#[test]
fn every_component_type_classifies_with_schema_defaults() {
    let cases = [
        (json!({"Light":{"kind":"spot"}}), "Light", true),
        (json!({"Camera":{}}), "Camera", true),
        (json!({"AudioSource":{}}), "AudioSource", true),
        (json!({"ReverbZone":{}}), "ReverbZone", true),
        (json!({"AcousticPortal":{}}), "AcousticPortal", false),
        (
            json!({"Collider":{"kind":"capsule","radius":0.5,"halfHeight":1}}),
            "Collider",
            false,
        ),
        (
            json!({"CharacterController":{}}),
            "CharacterController",
            false,
        ),
        (json!({"Joint":{"kind":"hinge"}}), "Joint", false),
        (json!({"NavAgent":{}}), "NavAgent", false),
        (json!({"NavObstacle":{}}), "NavObstacle", false),
        (
            json!({"NavLink":{"start":[0,0,0],"end":[4,0,0]}}),
            "NavLink",
            false,
        ),
        (
            json!({"NavMeshSurface":{"min":[-5,0,-5],"max":[5,2,5]}}),
            "NavMeshSurface",
            false,
        ),
        (json!({"Perceiver":{}}), "Perceiver", false),
    ];
    for (components, kind, iconed) in cases {
        let (parts, icon) = parts::classify(&record(components.clone()));
        assert!(
            parts.iter().any(|p| p.kind == kind),
            "{components} draws a {kind} shape"
        );
        assert_eq!(icon.is_some(), iconed, "{components} icon");
        assert!(TYPES.iter().any(|t| t.key == kind), "{kind} is a menu type");
    }
    // Icon-only types and the first-iconed-wins rule.
    let (_, icon) = parts::classify(&record(json!({"HeightFog":{},"VisualEffect":{}})));
    assert_eq!(icon.map(|i| i.glyph), Some("cloud-fog"));
    let (_, icon) = parts::classify(&record(json!({"Light":{"color":[1,0.5,0,1]}})));
    assert!(matches!(
        icon.map(|i| i.tint),
        Some(parts::Tint::Light([r, _, b])) if r == 1. && b == 0.
    ));
}

#[test]
fn settings_switch_types_icons_and_everything() {
    let mut state = state(&[]);
    let defaults = Settings::read(&state);
    assert!(defaults.show && defaults.icons && defaults.icons_3d && defaults.shows("Light"));
    state.settings.insert(
        "viewport.gizmoTypes".into(),
        json!({"Light":"off","Camera":"on"}),
    );
    state
        .settings
        .insert("viewport.iconSize".into(), json!(150));
    let settings = Settings::read(&state);
    assert!(!settings.shows("Light") && settings.shows("Camera"));
    assert_eq!(settings.icon_scale, 1.5);

    let scene = view(&[
        ("e_light1", record(json!({"Light":{}}))),
        ("e_camera1", record(json!({"Camera":{}}))),
    ]);
    let mut gizmos = Gizmos::default();
    let shown = draw(&mut gizmos, &scene, &state);
    assert!(gizmos.drawn.iter().all(|(e, ..)| e == "e_camera1"));
    assert!(!shown.is_empty());
    state
        .settings
        .insert("viewport.gizmos".into(), json!(false));
    assert!(draw(&mut gizmos, &scene, &state).is_empty());
    assert!(gizmos.at([400., 300.]).is_none());
}

#[test]
fn selection_draws_full_detail_and_unselected_entities_a_hint() {
    let scene = view(&[(
        "e_light1",
        record(json!({"Light":{"kind":"point","range":4}})),
    )]);
    let mut gizmos = Gizmos::default();
    let subtle = lines(&draw(&mut gizmos, &scene, &state(&[])));
    let full = lines(&draw(&mut gizmos, &scene, &state(&["e_light1"])));
    assert!(subtle > 0, "an unselected point light hints its range");
    assert!(
        full > subtle * 3,
        "the selection draws the sphere: {full} vs {subtle}"
    );
    // Colliders draw only selected, as in Unity.
    let scene = view(&[(
        "e_box1",
        record(json!({"Collider":{"kind":"box","halfExtents":[1,1,1]}})),
    )]);
    assert_eq!(lines(&draw(&mut gizmos, &scene, &state(&[]))), 0);
    // Twelve box edges, each with its halo.
    assert_eq!(lines(&draw(&mut gizmos, &scene, &state(&["e_box1"]))), 24);
}

#[test]
fn icons_pick_nearest_first_and_fade_near_the_camera() {
    let scene = view(&[
        ("e_far1", record(json!({"Light":{},"Transform":{}}))),
        (
            "e_near1",
            record(json!({"Camera":{},"Transform":{"position":[0,0,0.01]}})),
        ),
    ]);
    let mut gizmos = Gizmos::default();
    draw(&mut gizmos, &scene, &state(&[]));
    let (_, centre, radius) = gizmos.drawn.last().expect("icons drawn").clone();
    assert!(radius > 5.);
    assert_eq!(gizmos.at(centre), Some("e_near1"));
    assert_eq!(gizmos.at([centre[0] + radius + 6., centre[1]]), None);
    assert_eq!(billboard::near_fade(0.2), 0.);
    assert_eq!(billboard::near_fade(5.), 1.);
    // 3D icons shrink with distance, within limits; flat ones keep one size.
    let mut settings = Settings::read(&state(&[]));
    assert!(billboard::radius(&settings, 200.) < billboard::radius(&settings, 5.));
    settings.icons_3d = false;
    assert_eq!(
        billboard::radius(&settings, 200.),
        billboard::radius(&settings, 5.)
    );
}
