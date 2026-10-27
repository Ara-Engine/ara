//! Projection, persistence and attributed bookmark navigation through real editor services.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::{Editor, view_camera::SavedCameras};
use ara_proto::Endpoint;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[test]
fn orthographic_rays_are_parallel_and_preserve_near_plane_origins_at_depth() {
    for (yaw, pitch) in [(0., 0.), (1.3, 0.4), (0., 1.54)] {
        let camera = Camera {
            projection: CameraProjection::Orthographic,
            yaw,
            pitch,
            target: Vec3::ZERO,
            distance: 20.,
            orthographic_height: 6.,
            ..Camera::default()
        };
        for size in [[1200, 600], [600, 1200]] {
            let (right, up, forward) = camera.basis().unwrap();
            let (centre, direction) = camera.ray(size, [0., 0.]).unwrap();
            let (corner, parallel) = camera.ray(size, [0.5, -0.5]).unwrap();
            assert!((direction - forward).length() < 1e-5);
            assert!((parallel - direction).length() < 1e-5);
            assert!(
                (centre
                    - camera.position()
                    - forward * ara_render::scene::CameraSpec::ORTHOGRAPHIC_NEAR)
                    .length()
                    < 1e-5
            );
            let expected = right * (1.5 * size[0] as f32 / size[1] as f32) - up * 1.5;
            assert!((corner - centre - expected).length() < 1e-5);
            let view = camera.spec().resolve(size).unwrap();
            let vp = view.projection * view.view;
            for depth in [0.1, 10., 10_000.] {
                let clip = vp * (corner + parallel * depth).extend(1.);
                let ndc = clip.truncate() / clip.w;
                assert!(
                    (ndc.x - 0.5).abs() < 0.001 && (ndc.y + 0.5).abs() < 0.001,
                    "{ndc:?}"
                );
            }
        }
    }
    let perspective = Camera::default();
    let (eye, centre) = perspective.ray([800, 600], [0., 0.]).unwrap();
    let (same_eye, edge) = perspective.ray([800, 600], [0.8, 0.]).unwrap();
    assert_eq!(eye, same_eye);
    assert!(centre.dot(edge) < 0.99);
}

#[test]
fn projection_toggle_preserves_pivot_scale_and_invalid_pointer_inputs_are_rejected() {
    let mut camera = Camera::default();
    let original = camera;
    let before = camera.world_per_pixel(camera.target, 600.).unwrap();
    camera.set_projection(CameraProjection::Orthographic);
    assert!((camera.world_per_pixel(camera.target, 600.).unwrap() - before).abs() < 1e-6);
    let near_scale = camera.world_per_pixel(camera.target, 600.).unwrap();
    assert_eq!(
        camera
            .world_per_pixel(camera.target + camera.basis().unwrap().2 * 1000., 600.)
            .unwrap(),
        near_scale
    );
    let distance = camera.distance;
    camera.zoom(2.);
    assert_eq!(camera.distance, distance);
    assert!(
        camera.orthographic_height
            < 2. * original.distance * (original.fov_y_degrees.to_radians() * 0.5).tan()
    );
    camera.zoom(-2.);
    camera.set_projection(CameraProjection::Perspective);
    assert!((camera.distance - original.distance).abs() < 1e-4);
    for size in [[0, 600], [800, 0]] {
        assert!(camera.ray(size, [0., 0.]).is_none());
    }
    assert!(camera.ray([800, 600], [f32::NAN, 0.]).is_none());
    assert!(camera.world_per_pixel(Vec3::ZERO, f32::INFINITY).is_none());
    camera.projection = CameraProjection::Orthographic;
    camera.orthographic_height = f32::NAN;
    assert!(camera.ray([800, 600], [0., 0.]).is_none());
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target().join("camera-navigation").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Camera navigation","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_camera01","records":{}}),
            ),
            (
                "levels/other.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_camera02","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn preferences(&self) -> PathBuf {
        self.0.join("profile/preferences.json")
    }
    fn open(&self) -> Editor {
        Editor::open_with(&self.0, Some(self.preferences())).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn history(editor: &mut Editor) -> Value {
    editor.action("editor.history", json!({})).unwrap()["items"].clone()
}

#[test]
fn bookmarks_are_per_scene_typed_navigation_edits_and_recall_restores_the_full_lens() {
    let project = Project::new();
    let mut editor = project.open();
    editor.action("editor.camera", json!({"projection":"orthographic","orthographicHeight":3.5,"target":[1.,2.,3.],"yaw":1.1,"pitch":0.3})).unwrap();
    let saved = editor.state.camera;
    let before = history(&mut editor).as_array().unwrap().len();
    let authored_before = editor.host.store().revision();
    editor
        .action(
            "editor.camera.bookmark",
            json!({"action":"save","name":"Detail"}),
        )
        .unwrap();
    assert_eq!(history(&mut editor).as_array().unwrap().len(), before + 1);
    assert_eq!(editor.host.store().revision(), authored_before);
    let listed = editor
        .action("editor.camera.bookmark", json!({"action":"list"}))
        .unwrap();
    assert_eq!(listed["bookmarks"][0]["name"], "Detail");
    assert_eq!(history(&mut editor).as_array().unwrap().len(), before + 1);
    editor
        .action("editor.scene.open", json!({"scene":"levels/other.scene"}))
        .unwrap();
    assert!(
        editor
            .action("editor.camera.bookmark", json!({"action":"list"}))
            .unwrap()["bookmarks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    editor
        .action(
            "editor.camera",
            json!({"projection":"perspective","target":[90.,1.,0.]}),
        )
        .unwrap();
    editor
        .action(
            "editor.camera.bookmark",
            json!({"action":"save","name":"Detail"}),
        )
        .unwrap();
    editor
        .action(
            "editor.camera.bookmark",
            json!({"action":"recall","scene":"levels/main.scene","name":"Detail"}),
        )
        .unwrap();
    assert_eq!(editor.state.doc, "sc_camera01");
    assert_eq!(editor.state.camera, saved);
    let unchanged = editor.state.clone();
    let history_before = history(&mut editor);
    for args in [
        json!({"action":"recall","name":"Deleted"}),
        json!({"action":"delete","name":"Deleted"}),
        json!({"action":"save","name":"  "}),
        json!({"action":"recall","name":"Detail","scene":"levels/missing.scene"}),
    ] {
        assert!(editor.action("editor.camera.bookmark", args).is_err());
        assert_eq!(editor.state, unchanged);
        assert_eq!(history(&mut editor), history_before);
    }
    editor
        .action(
            "editor.camera.bookmark",
            json!({"action":"delete","name":"Detail"}),
        )
        .unwrap();
    assert!(!editor.state.camera_bookmarks.contains_key("sc_camera01"));
    assert!(editor.state.camera_bookmarks["sc_camera02"].contains_key("Detail"));
}

#[test]
fn agent_bookmark_recall_preserves_human_navigation_and_retains_attribution() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.camera",
            json!({"projection":"orthographic","orthographicHeight":8.}),
        )
        .unwrap();
    editor
        .action(
            "editor.camera.bookmark",
            json!({"action":"save","name":"Plan"}),
        )
        .unwrap();
    let bookmarked = editor.state.camera;
    editor
        .action("editor.scene.open", json!({"scene":"levels/other.scene"}))
        .unwrap();
    editor
        .action("editor.camera", json!({"target":[9.,2.,1.]}))
        .unwrap();
    let human = editor.state.clone();
    let session = Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"bookmark-agent","author":"agent:bookmark-agent"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    Endpoint::call(
        &mut editor,
        "editor.camera.bookmark",
        json!({"session":session,"action":"recall","scene":"levels/main.scene","name":"Plan"}),
    )
    .unwrap();
    editor.tick();
    assert_eq!(editor.state.doc, human.doc);
    assert_eq!(editor.state.camera, human.camera);
    assert_eq!(editor.state.selection, human.selection);
    assert_eq!(editor.state.scene_cameras, human.scene_cameras);
    assert_eq!(editor.state.presence[&session].camera, bookmarked);
    assert_eq!(editor.state.presence[&session].doc, "sc_camera01");
    assert_eq!(
        history(&mut editor).as_array().unwrap().last().unwrap()["author"],
        json!(ara_ops::Author::Agent("bookmark-agent".into()))
    );
}

#[test]
fn saved_views_read_old_cameras_and_persist_bookmarks_without_replacing_other_projects() {
    let project = Project::new();
    let profile = project.preferences();
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    let key = std::path::absolute(&project.0)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let old = Camera {
        yaw: 0.9,
        ..Camera::default()
    };
    let mut old_json = json!(old);
    old_json.as_object_mut().unwrap().remove("projection");
    old_json
        .as_object_mut()
        .unwrap()
        .remove("orthographicHeight");
    let path = profile.parent().unwrap().join("scene-cameras.json");
    let other = json!({"sc_elsewhere":{"yaw":0.2}});
    std::fs::write(
        &path,
        ara_json::to_canonical_string(
            &json!({"projects":{(key.clone()):{"sc_camera01":old_json},"another-project":other}}),
        ),
    )
    .unwrap();
    let mut views = SavedCameras::open(Some(&profile), &project.0);
    assert_eq!(views.saved["sc_camera01"], old);
    let camera = Camera {
        projection: CameraProjection::Orthographic,
        orthographic_height: 7.,
        ..old
    };
    let state = EditorState {
        doc: "sc_camera01".into(),
        camera,
        camera_bookmarks: BTreeMap::from([(
            "sc_camera01".into(),
            BTreeMap::from([("Plan".into(), camera)]),
        )]),
        ..EditorState::default()
    };
    views.observe(&state, true);
    drop(views);
    let views = SavedCameras::open(Some(&profile), &project.0);
    assert_eq!(views.saved["sc_camera01"], camera);
    assert_eq!(views.bookmarks["sc_camera01"]["Plan"], camera);
    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["projects"]["another-project"], other);
    drop(views);
}

#[test]
fn closing_a_floating_scene_redocks_it_without_closing_other_floating_tabs() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"scene","slot":"float","rect":[50.,60.,600.,450.]}),
        )
        .unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"inspector","slot":"float","rect":[700.,60.,300.,450.]}),
        )
        .unwrap();
    let inspector = editor
        .state
        .floating
        .iter()
        .find(|window| {
            DockLayout::new(window.root.clone())
                .unwrap()
                .contains("inspector")
        })
        .unwrap()
        .clone();
    editor
        .action("editor.layout.close", json!({"panel":"scene"}))
        .unwrap();
    assert!(
        DockLayout::new(editor.state.layout.clone())
            .unwrap()
            .contains("scene")
    );
    assert!(editor.state.floating.contains(&inspector));
    assert!(editor.state.dock().unwrap().contains("inspector"));
    let unchanged = editor.state.clone();
    assert!(
        editor
            .action("editor.layout.close", json!({"panel":"scene"}))
            .is_err()
    );
    assert_eq!(editor.state, unchanged);
}
