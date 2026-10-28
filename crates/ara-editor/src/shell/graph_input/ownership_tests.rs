//! Native graph consumption must protect pending callbacks without a scene edit.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target().join("graph-ownership").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!root.exists());
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                PathBuf::from("ara.project.json"),
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Graph ownership","main":"levels/main.scene"}),
            ),
            (
                PathBuf::from(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
                json!({"$schema":"ara://schema/scene/1","id":"sc_graphowner","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }

    fn open(&self) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        let resource = editor
            .action(
                "editor.resource.create",
                json!({"path":"owned.animation","domain":"animation"}),
            )
            .unwrap()["doc"]
            .clone();
        editor
            .action("editor.workspace", json!({"resource":resource,"asset":""}))
            .unwrap();
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"animation","slot":"view"}),
            )
            .unwrap();
        editor
            .action("editor.focus", json!({"panel":"animation"}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
        shell.frame().unwrap();
        assert_eq!(shell.graph_page, "animation");
        let rect = shell.graph_rect().unwrap();
        assert!(rect.w > 100. && rect.h > 100.);
        assert!(shell.active_graph().is_some());
        assert_eq!(shell.panel_failure_count(), 0);
        shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(crate::test_target().join("graph-ownership"))
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scene_context(state: &crate::state::EditorState) -> Value {
    json!({"doc":state.doc,"selection":state.selection,"focus":state.focus,"focusField":state.focus_field,"resource":state.resource,"camera":state.camera})
}

fn owner(editor: &Editor) -> crate::interaction::NavigationOwner {
    editor
        .interaction
        .capture(&editor.state, &editor.session, false)
}

fn assert_input_owned(pending: &crate::interaction::NavigationOwner, editor: &Editor) {
    assert!(
        pending
            .suppression(&editor.interaction, &editor.state, &editor.session)
            .is_some_and(|reason| reason.contains("input"))
    );
}

#[test]
fn graph_wheel_owns_input_while_hover_and_unclaimed_release_do_not() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let before = scene_context(&editor.state);
    let revision = editor.host.store().revision();
    let epoch = editor.interaction.revision();
    let point = shell.pointer;
    assert!(
        !shell
            .graph_event(
                &mut editor,
                &Event::MouseMotion {
                    position: point,
                    delta: [1., 1.]
                }
            )
            .unwrap()
    );
    assert!(
        !shell
            .graph_event(
                &mut editor,
                &Event::MouseButton {
                    button: 2,
                    pressed: false
                }
            )
            .unwrap()
    );
    assert_eq!(editor.interaction.revision(), epoch);
    let pending = owner(&editor);
    let zoom = editor.state.canvas.zoom;
    assert!(
        shell
            .graph_event(&mut editor, &Event::Wheel([0., 1.]))
            .unwrap()
    );
    assert_ne!(editor.state.canvas.zoom, zoom);
    assert_input_owned(&pending, &editor);
    assert_eq!(scene_context(&editor.state), before);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn graph_press_held_motion_and_escape_take_ownership_before_they_return() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let before = scene_context(&editor.state);
    let revision = editor.host.store().revision();
    let pressed = owner(&editor);
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::MouseButton {
                    button: 2,
                    pressed: true
                }
            )
            .unwrap()
    );
    assert_input_owned(&pressed, &editor);
    assert!(shell.canvas_dragging);
    let moving = owner(&editor);
    let position = [shell.pointer[0] + 24., shell.pointer[1] + 16.];
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::MouseMotion {
                    position,
                    delta: [24., 16.]
                }
            )
            .unwrap()
    );
    assert_input_owned(&moving, &editor);
    let cancelling = owner(&editor);
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::Key {
                    scancode: 41,
                    pressed: true,
                    repeat: false
                }
            )
            .unwrap()
    );
    assert_input_owned(&cancelling, &editor);
    assert!(!shell.canvas_dragging);
    assert_eq!(scene_context(&editor.state), before);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn graph_release_does_not_note_input_and_changed_canvas_still_guards_late_hints() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let before = scene_context(&editor.state);
    let revision = editor.host.store().revision();
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::MouseButton {
                    button: 2,
                    pressed: true
                }
            )
            .unwrap()
    );
    let position = [shell.pointer[0] + 24., shell.pointer[1] + 16.];
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::MouseMotion {
                    position,
                    delta: [24., 16.]
                }
            )
            .unwrap()
    );
    let pending = owner(&editor);
    let epoch = editor.interaction.revision();
    let canvas = editor.state.canvas;
    assert!(
        shell
            .graph_event(
                &mut editor,
                &Event::MouseButton {
                    button: 2,
                    pressed: false
                }
            )
            .unwrap()
    );
    assert_eq!(editor.interaction.revision(), epoch);
    assert_ne!(editor.state.canvas, canvas);
    assert!(
        pending
            .suppression(&editor.interaction, &editor.state, &editor.session)
            .is_some_and(|reason| reason.contains("navigation"))
    );
    assert_eq!(scene_context(&editor.state), before);
    assert_eq!(editor.host.store().revision(), revision);
}
