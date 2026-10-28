//! Real native-instance bounds, reflected colliders, and shared retained text.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_doc::{AssetId, RecordId, prefab::ResolvedEntity};
use glam::{Mat4, Quat};
use std::sync::Arc;
mod benchmark;
mod stats;
mod volume;
const RECT: Box2 = Box2 {
    x: 0.,
    y: 0.,
    w: 800.,
    h: 600.,
};

fn view(records: Vec<(&str, Value, Option<&str>)>) -> DocumentView {
    let entities = records
        .into_iter()
        .map(|(id, value, parent)| {
            (
                id.into(),
                ResolvedEntity {
                    record: value.as_object().unwrap().clone().into_iter().collect(),
                    parent: parent.map(str::to_owned),
                    order: None,
                    source: (
                        AssetId::parse("sc_diagnostic1").unwrap(),
                        RecordId::parse(id).unwrap(),
                    ),
                },
            )
        })
        .collect();
    DocumentView::with_imports(Arc::new(entities), Arc::default())
}
fn state(bounds: bool, colliders: bool) -> EditorState {
    let mut state = EditorState {
        camera: Camera {
            target: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
            distance: 20.,
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .settings
        .insert("viewport.bounds".into(), json!(bounds));
    state
        .settings
        .insert("viewport.colliders".into(), json!(colliders));
    state
}
fn sheet() -> Sheet {
    Sheet::parse(":root {--info-foreground:#80bbff;--success-foreground:#80ddaa;--warning-foreground:#ffcc66;}").unwrap()
}
fn scene(world: Mat4) -> ara_render::instanced::Scene {
    use ara_render::instanced::*;
    Scene {
        geometry: vec![Geometry {
            key: [7; 32],
            vertices: Vec::new(),
            lods: Vec::new(),
            bounds: [Vec3::new(-1., -0.5, -0.25), Vec3::new(1., 0.5, 0.25)],
            deforming: false,
            revision: 0,
        }],
        instances: vec![Instance {
            entity: 7,
            name: "diagnostic fixture".into(),
            geometry: 0,
            material: 0,
            world,
            previous_world: world,
        }],
        materials: vec![Material::default()],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: Vec::new(),
        environment_size: [0, 0],
    }
}
fn source(scene: &ara_render::instanced::Scene, key: SourceKey) -> Source<'_> {
    Source {
        scene: Some(scene),
        frame: None,
        key,
        current: true,
    }
}
fn extent(wires: &[Segment]) -> [Vec3; 2] {
    wires.iter().flat_map(|line| [line.from, line.to]).fold(
        [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)],
        |[min, max], point| [min.min(point), max.max(point)],
    )
}

#[test]
fn native_bounds_use_real_geometry_and_mirrored_rotated_instance_transform() {
    let view = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"MeshRenderer":{"size":[1,1,1]}}),
        None,
    )]);
    let scene = scene(Mat4::from_scale_rotation_translation(
        Vec3::new(-2., 3., 4.),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        Vec3::new(10., 0., 0.),
    ));
    let mut wires = Vec::new();
    let mut counts = Counts::default();
    bounds::collect(
        &mut wires,
        &mut counts,
        &view,
        source(&scene, SourceKey::default()),
    );
    assert_eq!(counts.mesh_bounds, 1);
    assert_eq!(wires.len(), 12);
    let [min, max] = extent(&wires);
    assert!(min.abs_diff_eq(Vec3::new(8.5, -2., -1.), 1e-5));
    assert!(max.abs_diff_eq(Vec3::new(11.5, 2., 1.), 1e-5));
}

#[test]
fn unchanged_input_reuses_geometry_and_projection_while_camera_and_pose_invalidate_separately() {
    let view = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"MeshRenderer":{}}),
        None,
    )]);
    let mut state = state(true, false);
    let mut scene = scene(Mat4::IDENTITY);
    let mut diagnostic = Diagnostics::default();
    let theme = sheet();
    let mut overlays = Vec::new();
    let mut key = SourceKey::default();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            source(&scene, key),
            &theme,
        )
        .unwrap();
    assert!(!overlays.is_empty());
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            source(&scene, key),
            &theme,
        )
        .unwrap();
    assert_eq!(
        (diagnostic.geometry_builds, diagnostic.projection_builds),
        (1, 1)
    );
    state.camera.yaw = 0.2;
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            source(&scene, key),
            &theme,
        )
        .unwrap();
    assert_eq!(
        (diagnostic.geometry_builds, diagnostic.projection_builds),
        (1, 2)
    );
    scene.instances[0].world = Mat4::from_translation(Vec3::X * 3.);
    key.pose_revision = Some(1);
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &view,
            &state,
            RECT,
            source(&scene, key),
            &theme,
        )
        .unwrap();
    assert_eq!(
        (diagnostic.geometry_builds, diagnostic.projection_builds),
        (2, 3)
    );
    assert!(extent(&diagnostic.wires)[0].x > 1.9);
}

#[test]
fn current_authoring_and_visibility_keys_update_colliders_while_native_content_is_stale() {
    let first = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"Collider":{}}),
        None,
    )]);
    let second = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{"position":[5,0,0]},"Collider":{}}),
        None,
    )]);
    let mut state = state(false, true);
    let mut diagnostic = Diagnostics::default();
    let mut overlays = Vec::new();
    let theme = sheet();
    let mut key = SourceKey {
        document_revision: 1,
        ..Default::default()
    };
    diagnostic
        .append(
            &mut overlays,
            &first,
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
    assert!(extent(&diagnostic.wires)[1].x < 1.);
    key.document_revision = 2;
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &second,
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
    assert!(extent(&diagnostic.wires)[0].x > 4.);
    state.hidden.insert("e_diagnostic1".into());
    key.scene = 1;
    overlays.clear();
    diagnostic
        .append(
            &mut overlays,
            &second,
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
}

#[test]
fn loading_model_shapes_and_stale_native_content_never_claim_real_model_bounds() {
    let view = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"MeshRenderer":{"model":"@assets/loading.glb#as_loading1"}}),
        None,
    )]);
    let scene = scene(Mat4::IDENTITY);
    let mut frame = ara_runtime::extract::RenderFrame::default();
    frame.draws.push(ara_runtime::extract::Draw {
        entity: 7,
        address: "e_diagnostic1".into(),
        label: "loading".into(),
        geometry: ara_runtime::extract::Geometry::Shape {
            shape: ara_runtime::components::MeshShape::Box,
            size: [1.; 3],
        },
        world: glam34::Mat4::IDENTITY,
        color: [1.; 4],
        roughness: 0.5,
        metallic: 0.,
        emissive: [0.; 3],
        emissive_factor: None,
        material: None,
        skin: None,
    });
    let mut wires = Vec::new();
    let mut counts = Counts::default();
    bounds::collect(
        &mut wires,
        &mut counts,
        &view,
        Source {
            frame: Some(&frame),
            ..source(&scene, SourceKey::default())
        },
    );
    assert!(wires.is_empty());
    assert_eq!(counts.pending_models, 1);
    bounds::collect(
        &mut wires,
        &mut counts,
        &view,
        Source {
            current: false,
            ..source(&scene, SourceKey::default())
        },
    );
    assert!(wires.is_empty());
    assert_eq!(counts.unavailable_bounds, 1);
}

#[test]
fn reflected_box_uses_parent_body_pose_and_physics_dimensions_instead_of_visual_scale() {
    let view = view(vec![
        (
            "e_parentdiag1",
            json!({"Transform":{"position":[5,10,0],"rotation":[0,0,90],"scale":[2,3,4]}}),
            None,
        ),
        (
            "e_diagnostic1",
            json!({"Transform":{"position":[1,2,0]},"Collider":{"center":[1,0,0],"halfExtents":[0.5,1,2]}}),
            Some("e_parentdiag1"),
        ),
    ]);
    let mut wires = Vec::new();
    let mut counts = Counts::default();
    collider::collect(&mut wires, &mut counts, &view, &state(false, true));
    assert_eq!(counts.colliders, 1);
    assert_eq!(wires.len(), 12);
    let [min, max] = extent(&wires);
    assert!(min.abs_diff_eq(Vec3::new(-2., 12.5, -2.), 1e-4));
    assert!(max.abs_diff_eq(Vec3::new(0., 13.5, 2.), 1e-4));
}

#[test]
fn sphere_capsule_cylinder_sensor_and_unavailable_cooked_shapes_use_their_real_contract() {
    for (kind, half_y) in [("sphere", 2.), ("capsule", 5.), ("cylinder", 3.)] {
        let view = view(vec![(
            "e_diagnostic1",
            json!({"Transform":{},"Collider":{"kind":kind,"radius":2.,"halfHeight":3.,"sensor":true}}),
            None,
        )]);
        let mut wires = Vec::new();
        let mut counts = Counts::default();
        collider::collect(&mut wires, &mut counts, &view, &state(false, true));
        let [min, max] = extent(&wires);
        assert!(min.abs_diff_eq(Vec3::new(-2., -half_y, -2.), 1e-5));
        assert!(max.abs_diff_eq(Vec3::new(2., half_y, 2.), 1e-5));
        assert_eq!((counts.colliders, counts.sensors), (1, 1));
        assert!(wires.iter().all(|line| matches!(line.tint, Tint::Sensor)));
    }
    for kind in ["cooked", "modelConvex", "modelMesh"] {
        let view = view(vec![(
            "e_diagnostic1",
            json!({"Transform":{},"Collider":{"kind":kind}}),
            None,
        )]);
        let mut wires = Vec::new();
        let mut counts = Counts::default();
        collider::collect(&mut wires, &mut counts, &view, &state(false, true));
        assert!(wires.is_empty());
        assert_eq!(counts.unsupported_colliders, 1);
    }
}

#[test]
fn malformed_collider_and_nonfinite_native_bounds_are_skipped_without_poisoning_lines() {
    let view = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"Collider":{"kind":"sphere","radius":-1.}}),
        None,
    )]);
    let mut wires = Vec::new();
    let mut counts = Counts::default();
    collider::collect(&mut wires, &mut counts, &view, &state(false, true));
    assert!(wires.is_empty());
    assert_eq!(counts.invalid_colliders, 1);
    let mut scene = scene(Mat4::IDENTITY);
    scene.geometry[0].bounds[0].x = f32::NAN;
    bounds::collect(
        &mut wires,
        &mut counts,
        &view,
        source(&scene, SourceKey::default()),
    );
    assert!(wires.is_empty());
    assert_eq!(counts.invalid_bounds, 1);
}

#[test]
fn perspective_and_orthographic_near_plane_segments_clip_to_finite_viewport_coordinates() {
    for projection_kind in [
        crate::state::CameraProjection::Perspective,
        crate::state::CameraProjection::Orthographic,
    ] {
        let mut state = state(false, false);
        state.camera.projection = projection_kind;
        let eye = state.camera.position();
        let (_, _, forward) = state.camera.basis().unwrap();
        let wires = [
            Segment {
                from: eye - forward,
                to: eye + forward * 3. + Vec3::X,
                tint: Tint::Bounds,
            },
            Segment {
                from: Vec3::NEG_X * 1000.,
                to: Vec3::X * 1000.,
                tint: Tint::Collider,
            },
            Segment {
                from: eye - forward * 2.,
                to: eye - forward,
                tint: Tint::Sensor,
            },
        ];
        let mut lines = Vec::new();
        let mut counts = Counts::default();
        projection::project(&wires, &mut lines, &mut counts, &state.camera, RECT);
        assert!(!lines.is_empty());
        assert!(counts.skipped_lines >= 1);
        for line in lines {
            for point in [line.from, line.to] {
                assert!(point.into_iter().all(f32::is_finite));
                assert!(
                    point[0] >= RECT.x - 1e-4
                        && point[0] <= RECT.x + RECT.w + 1e-4
                        && point[1] >= RECT.y - 1e-4
                        && point[1] <= RECT.y + RECT.h + 1e-4
                );
            }
        }
    }
}
