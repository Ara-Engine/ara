//! Selected local cages are cached gizmos, independent of Bounds/Collider toggles.
use super::*;

#[test]
fn selected_local_volume_cages_share_the_cache_and_follow_gizmo_visibility() {
    let view = view(vec![
        (
            "e_volumebox1",
            json!({"Transform":{"scale":[2,4,6]},"PostProcessVolume":{"shape":"box","blendDistance":2.}}),
            None,
        ),
        (
            "e_volumesphere1",
            json!({"Transform":{"scale":[4,4,4]},"PostProcessVolume":{"shape":"sphere"}}),
            None,
        ),
        (
            "e_volumeglobal1",
            json!({"Transform":{},"PostProcessVolume":{"shape":"global"}}),
            None,
        ),
        ("e_volumefog1", json!({"Transform":{},"HeightFog":{}}), None),
    ]);
    let mut state = state(false, false);
    let mut diagnostic = Diagnostics::default();
    let mut overlays = Vec::new();
    let theme = sheet();
    let key = SourceKey::default();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &theme,
        )
        .unwrap();
    assert!(overlays.is_empty());
    state.selection = vec!["e_volumebox1".into()];
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &theme,
        )
        .unwrap();
    assert_eq!(diagnostic.wires.len(), 12);
    assert_eq!(diagnostic.counts.volumes, 1);
    let [min, max] = extent(&diagnostic.wires);
    assert!(min.abs_diff_eq(Vec3::new(-1., -2., -3.), 1e-5));
    assert!(max.abs_diff_eq(Vec3::new(1., 2., 3.), 1e-5));
    assert!(
        diagnostic
            .wires
            .iter()
            .all(|wire| matches!(wire.tint, Tint::Volume))
    );
    assert_eq!(
        diagnostic.report()["selectedVolumes"][0]["blendDistance"],
        2.
    );
    let builds = (diagnostic.geometry_builds, diagnostic.projection_builds);
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &theme,
        )
        .unwrap();
    assert_eq!(
        (diagnostic.geometry_builds, diagnostic.projection_builds),
        builds
    );
    state.selection = vec!["e_volumesphere1".into()];
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &theme,
        )
        .unwrap();
    assert_eq!(diagnostic.wires.len(), 96);
    assert_eq!(diagnostic.counts.volumes, 1);
    state.hide_gizmos = true;
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &theme,
        )
        .unwrap();
    assert!(diagnostic.wires.is_empty());
    assert!(overlays.is_empty());
    state.hide_gizmos = false;
    for selected in ["e_volumeglobal1", "e_volumefog1"] {
        state.selection = vec![selected.into()];
        overlays.clear();
        diagnostic
            .append(
                &mut overlays,
                &view,
                &state,
                RECT,
                Source {
                    scene: None,
                    frame: None,
                    key,
                    current: false,
                },
                &theme,
            )
            .unwrap();
        assert!(diagnostic.wires.is_empty());
        assert_eq!(diagnostic.counts.volumes, 0);
    }
}

fn append_selected(
    diagnostic: &mut Diagnostics,
    view: &DocumentView,
    state: &EditorState,
    key: SourceKey,
) -> Vec<Overlay> {
    let mut overlays = Vec::new();
    diagnostic
        .append(
            &mut overlays,
            view,
            state,
            RECT,
            Source {
                scene: None,
                frame: None,
                key,
                current: false,
            },
            &sheet(),
        )
        .unwrap();
    overlays
}
fn assert_box_corners(wires: &[Segment], centre: Vec3, rotation: Quat, half: Vec3) {
    let corners: Vec<_> = (0..8)
        .map(|bits| {
            centre
                + rotation
                    * (half
                        * Vec3::new(
                            if bits & 1 == 0 { -1. } else { 1. },
                            if bits & 2 == 0 { -1. } else { 1. },
                            if bits & 4 == 0 { -1. } else { 1. },
                        ))
        })
        .collect();
    assert!(
        wires
            .iter()
            .flat_map(|line| [line.from, line.to])
            .all(|point| corners
                .iter()
                .any(|corner| corner.abs_diff_eq(point, 0.001)))
    );
    assert!(corners.iter().all(|corner| {
        wires.iter().any(|line| {
            line.from.abs_diff_eq(*corner, 0.001) || line.to.abs_diff_eq(*corner, 0.001)
        })
    }));
}

#[test]
fn reflection_probe_cages_use_runtime_world_pose_abs_scale_and_actual_capture_point() {
    let view = view(vec![
        (
            "e_probeparent",
            json!({"Transform":{"position":[3,1,-2],"rotation":[0,90,0],"scale":[-2,3,4]}}),
            None,
        ),
        (
            "e_probechild",
            json!({"Transform":{"position":[1,2,3],"rotation":[25,35,0],"scale":[1.5,0.5,2]},
            "ReflectionProbe":{"bounds":[6,8,10],"projectionBounds":[12,0,20],"projectionOffset":[1,2,3],"captureOffset":[-0.5,0.25,1]}}),
            Some("e_probeparent"),
        ),
    ]);
    let mut state = state(false, false);
    state.selection = vec!["e_probechild".into()];
    let mut diagnostic = Diagnostics::default();
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    let world = crate::world::world_transform(view.entities(), "e_probechild").unwrap();
    let pose =
        ara_runtime::math::Pose::from_matrix(glam34::Mat4::from_cols_array(&world.to_cols_array()));
    let scale = Vec3::from_array(pose.scale.to_array())
        .abs()
        .max(Vec3::splat(1e-4));
    let rotation = Quat::from_array(pose.rotation.to_array()).normalize();
    let centre = Vec3::from_array(pose.translation.to_array());
    let half = Vec3::new(6., 8., 10.) * 0.5 * scale;
    assert_eq!(
        diagnostic.wires.len(),
        40,
        "12 solid + 24 dashed + 3 capture axes + connector"
    );
    assert_box_corners(&diagnostic.wires[..12], centre, rotation, half);
    // VQ falls back the WHOLE projection box when ANY dimension is zero.
    let projection_centre = centre + rotation * (Vec3::new(1., 2., 3.) * scale);
    let expected = ara_render::look::captures::CaptureSpec {
        position: centre,
        rotation,
        influence: half,
        blend: 1.,
        projection_centre: Vec3::new(1., 2., 3.) * scale,
        projection: half,
        capture_offset: Vec3::new(-0.5, 0.25, 1.) * scale,
        priority: 0,
        resolution: 256,
        intensity: 1.,
    };
    expected.validate().unwrap();
    // Dash endpoints include every projection corner, even though interior dash
    // endpoints are not themselves box corners.
    let inverse = rotation.conjugate();
    for wire in &diagnostic.wires[12..36] {
        for point in [wire.from, wire.to] {
            let local = inverse * (point - projection_centre);
            assert!(local.abs().cmple(half + Vec3::splat(0.001)).all());
            assert!((local.abs() - half).abs().min_element() < 0.001);
        }
    }
    let point = expected.capture_point();
    for wire in &diagnostic.wires[36..39] {
        assert!(((wire.from + wire.to) * 0.5).abs_diff_eq(point, 0.001));
    }
    assert!(diagnostic.wires[39].from.abs_diff_eq(centre, 0.001));
    assert!(diagnostic.wires[39].to.abs_diff_eq(point, 0.001));
    assert_eq!(
        diagnostic.counts.volumes, 0,
        "probes are not post-process volumes"
    );
    assert!(diagnostic.volumes.is_empty());
    let builds = (diagnostic.geometry_builds, diagnostic.projection_builds);
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert_eq!(
        (diagnostic.geometry_builds, diagnostic.projection_builds),
        builds,
        "idle reuses geometry and projection"
    );
}

#[test]
fn projection_box_uses_full_size_and_offset_or_whole_influence_fallback() {
    for (projection, min, max) in [
        ([8, 10, 12], Vec3::new(-3., -6., -4.), Vec3::new(5., 4., 8.)),
        ([8, 0, 12], Vec3::new(0., -3., -1.), Vec3::new(2., 1., 5.)),
    ] {
        let view = view(vec![(
            "e_probefallback",
            json!({"Transform":{},"ReflectionProbe":{
            "bounds":[2,4,6],"projectionBounds":projection,"projectionOffset":[1,-1,2],"captureOffset":[2,3,4]}}),
            None,
        )]);
        let mut state = state(false, false);
        state.selection = vec!["e_probefallback".into()];
        let mut diagnostic = Diagnostics::default();
        append_selected(&mut diagnostic, &view, &state, SourceKey::default());
        assert_eq!(diagnostic.wires.len(), 40);
        let [actual_min, actual_max] = extent(&diagnostic.wires[12..36]);
        assert!(actual_min.abs_diff_eq(min, 1e-5));
        assert!(actual_max.abs_diff_eq(max, 1e-5));
        for wire in &diagnostic.wires[36..39] {
            assert!(((wire.from + wire.to) * 0.5).abs_diff_eq(Vec3::new(2., 3., 4.), 1e-5));
        }
    }
}

#[test]
fn reflection_probe_visibility_validation_and_post_volume_counts_remain_truthful() {
    for component in [
        json!({"enabled":false}),
        json!({"captureResolution":999}),
        json!({"bounds":[0,1,1]}),
        json!({"captureOffset":[1e308,0,0]}),
    ] {
        let view = view(vec![(
            "e_probeinvalid",
            json!({"Transform":{},"ReflectionProbe":component}),
            None,
        )]);
        let mut state = state(false, false);
        state.selection = vec!["e_probeinvalid".into()];
        let mut diagnostic = Diagnostics::default();
        append_selected(&mut diagnostic, &view, &state, SourceKey::default());
        assert!(diagnostic.wires.is_empty());
        assert_eq!(diagnostic.counts.volumes, 0);
    }
    let view = view(vec![
        (
            "e_probevalid",
            json!({"Transform":{},"ReflectionProbe":{}}),
            None,
        ),
        (
            "e_probedegenerate",
            json!({"Transform":{"scale":[0,1,1]},"ReflectionProbe":{}}),
            None,
        ),
        (
            "e_probeunderflow",
            json!({"Transform":{"scale":[1e-30,1e15,1e15]},"ReflectionProbe":{}}),
            None,
        ),
        (
            "e_probesmall",
            json!({"Transform":{"scale":[0.00001,0.00001,0.00001]},"ReflectionProbe":{}}),
            None,
        ),
        (
            "e_probeboth",
            json!({"Transform":{},"ReflectionProbe":{},"PostProcessVolume":{"shape":"box","blendDistance":3}}),
            None,
        ),
    ]);
    let mut state = state(false, false);
    state.selection = vec!["e_probevalid".into()];
    let mut diagnostic = Diagnostics::default();
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert_eq!(
        diagnostic.wires.len(),
        15,
        "identical default projection does not double-paint its influence cage"
    );
    state.hidden.insert("e_probevalid".into());
    append_selected(
        &mut diagnostic,
        &view,
        &state,
        SourceKey {
            scene: 1,
            ..SourceKey::default()
        },
    );
    assert!(diagnostic.wires.is_empty());
    state.hidden.clear();
    state.selection = vec!["e_probedegenerate".into()];
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert!(diagnostic.wires.is_empty());
    state.selection = vec!["e_probeunderflow".into()];
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert!(diagnostic.wires.is_empty());
    state.selection = vec!["e_probesmall".into()];
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    let [min, max] = extent(&diagnostic.wires[..12]);
    assert!(min.abs_diff_eq(Vec3::splat(-0.005), 1e-6));
    assert!(max.abs_diff_eq(Vec3::splat(0.005), 1e-6));
    state.selection = vec!["e_probeboth".into()];
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert_eq!(diagnostic.wires.len(), 27);
    assert_eq!(diagnostic.counts.volumes, 1);
    assert_eq!(
        diagnostic.report()["selectedVolumes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    state.hide_gizmos = true;
    append_selected(&mut diagnostic, &view, &state, SourceKey::default());
    assert!(diagnostic.wires.is_empty());
}
