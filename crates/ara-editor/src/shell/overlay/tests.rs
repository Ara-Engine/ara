//! Real handle projection and paint/pick agreement, without an editor process or GPU.
use super::*;
use crate::state::CameraProjection;
use ara_doc::{AssetId, Record, RecordId, prefab::ResolvedEntity};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

mod benchmark;

const RECT: Box2 = Box2 {
    x: 265.,
    y: 64.,
    w: 852.,
    h: 571.,
};

fn camera(projection: CameraProjection, yaw: f32, pitch: f32) -> Camera {
    Camera {
        target: Vec3::ZERO,
        yaw,
        pitch,
        distance: 20.,
        projection,
        ..Camera::default()
    }
}

fn fixture(camera: Camera, pivot: Vec3, tool: Tool) -> Handles {
    let mut record = Record::new();
    record.insert("Transform".into(), json!({"position":pivot.to_array()}));
    let entities = BTreeMap::from([(
        "e_gizmopick1".into(),
        ResolvedEntity {
            record,
            parent: None,
            order: None,
            source: (
                AssetId::parse("sc_gizmopick1").expect("fixture scene ID"),
                RecordId::parse("e_gizmopick1").expect("fixture entity ID"),
            ),
        },
    )]);
    let view = DocumentView::with_imports(Arc::new(entities), Arc::default());
    let state = EditorState {
        camera,
        tool,
        selection: vec!["e_gizmopick1".into()],
        ..EditorState::default()
    };
    handles(&view, &state, RECT).expect("fixture must project real transform handles")
}

fn axis_point(handles: &Handles, axis: usize) -> [f32; 2] {
    (Vec2::from(handles.origin)
        + (Vec2::from(handles.ends[axis]) - Vec2::from(handles.origin)) * 0.65)
        .into()
}

fn centroid(quad: [[f32; 2]; 4]) -> [f32; 2] {
    (quad.into_iter().map(Vec2::from).sum::<Vec2>() / 4.).into()
}

#[test]
fn front_perspective_off_centre_x_axis_is_reachable_in_the_native_failure_geometry() {
    let handles = fixture(
        camera(CameraProjection::Perspective, 0., 0.),
        Vec3::new(-4., 0., 0.),
        Tool::Translate,
    );
    // Perspective makes Z appear over 20 px long even though the XZ square is
    // exactly edge-on. The former two-arm test alone accepted that zero-area quad.
    assert!(Vec2::from(handles.ends[2]).distance(Vec2::from(handles.origin)) > 20.);
    assert!(handles.plane_quad(1).is_none());
    let pointer = axis_point(&handles, 0);
    assert_eq!(hit(&handles, pointer), Some(Handle::Axis(0)));
    assert_eq!(hit(&handles, [pointer[0] + 10_000., pointer[1]]), None);
}

#[test]
fn axis_aligned_perspective_and_orthographic_views_keep_each_legible_axis_reachable() {
    for projection in [
        CameraProjection::Perspective,
        CameraProjection::Orthographic,
    ] {
        for (yaw, pitch, pivot) in [
            (0., 0., Vec3::new(-4., 0., 0.)),
            (std::f32::consts::FRAC_PI_2, 0., Vec3::new(0., 0., 4.)),
            (std::f32::consts::PI, 0., Vec3::new(4., 0., 0.)),
            (0., std::f32::consts::FRAC_PI_2, Vec3::new(-4., 0., 3.)),
        ] {
            for tool in [Tool::Translate, Tool::Scale] {
                let handles = fixture(camera(projection, yaw, pitch), pivot, tool);
                for axis in 0..3 {
                    if Vec2::from(handles.ends[axis]).distance(Vec2::from(handles.origin)) > 40. {
                        assert_eq!(
                            hit(&handles, axis_point(&handles, axis)),
                            Some(Handle::Axis(axis)),
                            "{projection:?}, yaw={yaw}, pitch={pitch}, {tool:?}, axis={axis}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn ordinary_planar_handles_keep_their_painted_square_and_pick_target() {
    for projection in [
        CameraProjection::Perspective,
        CameraProjection::Orthographic,
    ] {
        let handles = fixture(camera(projection, 0.6, 0.4), Vec3::ZERO, Tool::Translate);
        for axis in 0..3 {
            let quad = handles
                .plane_quad(axis)
                .expect("ordinary plane square remains visible");
            assert_eq!(hit(&handles, centroid(quad)), Some(Handle::Plane(axis)));
        }
    }
}

#[test]
fn convex_quad_validation_uses_local_area_and_retains_winding_and_hit_margin() {
    // Absolute-coordinate shoelace products lose a small square's area here.
    let quad = [
        [100_000., 100_000.],
        [100_020., 100_000.],
        [100_020., 100_020.],
        [100_000., 100_020.],
    ];
    for quad in [quad, [quad[3], quad[2], quad[1], quad[0]]] {
        let shape = PlaneQuad::new(quad).expect("translated square has a valid area");
        assert!(shape.contains(Vec2::new(100_010., 100_010.), 2.));
        assert!(shape.contains(Vec2::new(99_999., 100_010.), 2.));
        assert!(!shape.contains(Vec2::new(99_997., 100_010.), 2.));
        assert!(!shape.contains(Vec2::new(f32::NAN, 100_010.), 2.));
        assert!(!shape.contains(Vec2::new(100_010., 100_010.), f32::INFINITY));
    }
    for quad in [
        [[0., 0.]; 4],
        [[0., 0.], [10., 0.], [20., 0.], [30., 0.]],
        [[0., 0.], [20., 0.], [20., 0.01], [0., 0.01]],
        [[0., 0.], [20., 20.], [0., 20.], [20., 0.]],
        [[0., 0.], [20., 0.], [5., 5.], [0., 20.]],
        [[0., 0.], [20., 0.], [f32::NAN, 20.], [0., 20.]],
        [[0., 0.], [20., 0.], [f32::INFINITY, 20.], [0., 20.]],
    ] {
        assert!(
            PlaneQuad::new(quad).is_none(),
            "invalid or invisible plane: {quad:?}"
        );
    }
}

#[test]
fn paint_uses_the_same_finite_noncollapsed_plane_geometry_as_hit_testing() {
    let handles = fixture(
        camera(CameraProjection::Perspective, 0., 0.),
        Vec3::new(-4., 0., 0.),
        Tool::Translate,
    );
    let mut malformed = handles;
    malformed.ends[0] = [f32::NAN, f32::INFINITY];
    for (handles, gizmo) in [
        (handles, GizmoState::default()),
        (
            malformed,
            GizmoState {
                active: Some(Handle::Axis(0)),
                hover: None,
            },
        ),
    ] {
        let mut overlays = Vec::new();
        draw_handles(
            &mut overlays,
            &handles,
            gizmo,
            &|_, alpha| Ok([1., 1., 1., alpha]),
            true,
        )
        .expect("drawing needs no GPU resources");
        for overlay in overlays {
            match overlay {
                Overlay::Quad { corners, .. } => {
                    assert!(corners.into_iter().flatten().all(f32::is_finite));
                    let [a, b, c, d] = corners.map(Vec2::from);
                    let area = (b - a).perp_dot(c - a) + (c - a).perp_dot(d - a);
                    assert!(
                        area.abs() > 0.01,
                        "paint must not submit a collapsed plane square"
                    );
                }
                Overlay::Line {
                    from, to, width, ..
                } => {
                    assert!(
                        from.into_iter()
                            .chain(to)
                            .chain([width])
                            .all(f32::is_finite)
                    );
                }
                _ => {}
            }
        }
    }
    assert_eq!(hit(&handles, [f32::NAN, handles.origin[1]]), None);
    assert_eq!(hit(&handles, [handles.origin[0], f32::INFINITY]), None);
}
