//! Navigation tests call the production controller's integration step without a GPU/window.
#![allow(clippy::unwrap_used)]
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn open() -> (Editor, Shell) {
    let root = crate::test_target()
        .join("ux-navigation-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(root.join("levels")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Navigation","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_fly0001","records":{}}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    }
    let mut editor = Editor::open(&root).unwrap();
    editor
        .action("editor.focus", json!({"panel":"scene"}))
        .unwrap();
    let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    (editor, shell)
}
fn controller() -> Controller {
    let mut controller = Controller {
        fly_capture: true,
        ..Controller::default()
    };
    controller.buttons[3] = true;
    controller.keys.insert(26);
    controller
}
#[test]
fn equal_time_fly_obeys_camera_speed_and_normalizes_diagonal_input() {
    let (mut editor, mut shell) = open();
    let baseline = editor.state.camera;
    let mut measured = Vec::new();
    for speed in [1., 4., 16.] {
        editor.action("editor.camera",json!({"target":baseline.target.to_array(),"yaw":baseline.yaw,"pitch":baseline.pitch,"distance":baseline.distance})).unwrap();
        editor
            .action(
                "editor.settings.set",
                json!({"key":"viewport.cameraSpeed","value":speed}),
            )
            .unwrap();
        let mut controller = controller();
        let start = editor.state.camera.position();
        for _ in 0..120 {
            assert!(controller.fly(&mut editor, &shell, 1. / 120.).unwrap());
        }
        let distance = editor.state.camera.position().distance(start);
        assert!(
            (distance - speed as f32).abs() < 0.001,
            "speed {speed}, distance {distance}"
        );
        measured.push(distance);
        controller.keys.insert(7);
        let start = editor.state.camera.position();
        for _ in 0..60 {
            controller.fly(&mut editor, &shell, 1. / 60.).unwrap();
        }
        assert!((editor.state.camera.position().distance(start) - speed as f32).abs() < 0.001);
    }
    assert!((measured[1] / measured[0] - 4.).abs() < 0.001);
    assert!((measured[2] / measured[0] - 16.).abs() < 0.001);
    shell.modifiers[0] = true;
    let start = editor.state.camera.position();
    let mut controller = controller();
    for _ in 0..120 {
        controller.fly(&mut editor, &shell, 1. / 120.).unwrap();
    }
    assert!((editor.state.camera.position().distance(start) - 48.).abs() < 0.002);
}
#[test]
fn fly_requires_scene_capture_and_focus_and_yields_to_a_modal() {
    let (mut editor, mut shell) = open();
    let mut controller = controller();
    let camera = editor.state.camera;
    controller.fly_capture = false;
    assert!(!controller.fly(&mut editor, &shell, 0.1).unwrap());
    assert_eq!(editor.state.camera, camera);
    controller.fly_capture = true;
    editor
        .action("editor.focus", json!({"panel":"inspector"}))
        .unwrap();
    assert!(!controller.fly(&mut editor, &shell, 0.1).unwrap());
    assert_eq!(editor.state.camera, camera);
    editor
        .action("editor.focus", json!({"panel":"scene"}))
        .unwrap();
    shell.dialog = Some(crate::shell::Dialog::about());
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.modal_active());
    assert!(!controller.fly(&mut editor, &shell, 0.1).unwrap());
    assert_eq!(editor.state.camera, camera);
    for dt in [f32::NAN, f32::INFINITY, -1., 0.] {
        assert!(!controller.fly(&mut editor, &shell, dt).unwrap());
    }
}
#[test]
fn fly_look_preserves_eye_while_orbit_preserves_pivot() {
    let mut look = crate::state::Camera::default();
    let original = look;
    let eye = look.position();
    look.look([70., 30.]);
    assert!(look.position().abs_diff_eq(eye, 0.00001));
    assert_ne!(look.target, original.target);
    let mut orbit = original;
    orbit.orbit([70., 30.]);
    assert_eq!(orbit.target, original.target);
    assert!(!orbit.position().abs_diff_eq(eye, 0.1));
}
