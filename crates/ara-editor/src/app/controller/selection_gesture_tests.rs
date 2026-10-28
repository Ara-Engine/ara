//! Production gizmo preview/commit helpers require no native renderer to prove atomicity.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn open() -> (Editor, Shell) {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let root = target.join("ux-selection-gestures").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Native gesture helpers","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_gesture01","records":{}}),
        ),
    ] {
        std::fs::write(
            root.join(
                if path == "ara.project.json" || path.starts_with("settings/") {
                    std::path::PathBuf::from(path)
                } else {
                    std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
                },
            ),
            ara_json::to_canonical_string(&value),
        )
        .unwrap();
    }
    let mut editor = Editor::open(&root).unwrap();
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":"e_gesturea1","name":"A","components":{"Transform":{"position":[-2,0,0]},"FollowCamera":{"target":"e_gestureb1"}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_gesturechild","name":"Child","parent":"e_gesturea1","components":{"Transform":{"position":[0,1,0]}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_gestureb1","name":"B","components":{"Transform":{"position":[2,0,0]}}}}),
    ],"Create sources",None).unwrap();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_gesturea1","e_gesturechild","e_gestureb1"]}),
        )
        .unwrap();
    editor
        .action(
            "editor.tool",
            json!({"tool":"translate","snapping":false,"local":false}),
        )
        .unwrap();
    let shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    (editor, shell)
}
fn records(editor: &Editor) -> Value {
    json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .records()
    )
}
fn preview(editor: &mut Editor, shell: &mut Shell) -> Controller {
    let view = shell.document_view(editor).unwrap();
    let rect = shell.viewport().unwrap();
    let handles = overlay::handles(&view, &editor.state, rect).unwrap();
    let start = (Vec2::from(handles.origin)
        + (Vec2::from(handles.ends[0]) - Vec2::from(handles.origin)) * 0.65)
        .to_array();
    let end = (Vec2::from(start)
        + (Vec2::from(handles.ends[0]) - Vec2::from(handles.origin)).normalize() * 80.)
        .to_array();
    let mut controller = Controller::default();
    controller
        .begin_gizmo(
            editor,
            &handles,
            overlay::Handle::Axis(0),
            start,
            rect,
            true,
        )
        .unwrap();
    controller.drive_gizmo(editor, end, Some(rect)).unwrap();
    controller
}
#[test]
fn alt_gizmo_preview_keeps_sources_and_commits_creation_and_pose_as_one_undo() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    let mut controller = preview(&mut editor, &mut shell);
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
    let gesture = editor.gesture.as_ref().unwrap();
    assert_eq!(gesture.initial.len(), 2);
    assert_ne!(gesture.initial, gesture.current);
    controller.finish_gizmo(&mut editor).unwrap();
    let committed = records(&editor);
    assert_eq!(committed.as_object().unwrap().len(), 6);
    assert_eq!(editor.host.store().revision(), revision + 1);
    for source in ["e_gesturea1", "e_gestureb1", "e_gesturechild"] {
        assert_eq!(committed[source], before[source]);
    }
    let roots = &editor.state.selection;
    assert_eq!(roots.len(), 2);
    let a = roots
        .iter()
        .find(|id| committed[id.as_str()]["name"] == "A copy")
        .unwrap();
    let b = roots
        .iter()
        .find(|id| committed[id.as_str()]["name"] == "B copy")
        .unwrap();
    assert_eq!(committed[a.as_str()]["FollowCamera"]["target"], b.as_str());
    assert_ne!(
        committed[a.as_str()]["Transform"]["position"],
        before["e_gesturea1"]["Transform"]["position"]
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(records(&editor), before);
    editor.action("history.redo", json!({})).unwrap();
    assert_eq!(records(&editor), committed);
}
#[test]
fn unrelated_source_rename_survives_alt_duplicate_without_interrupting_the_gesture() {
    let (mut editor, mut shell) = open();
    let mut controller = preview(&mut editor, &mut shell);
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_gesturea1","path":"name","value":"Renamed during gesture"}})],"Concurrent source rename",None).unwrap();
    let current = records(&editor);
    let revision = editor.host.store().revision();
    controller.finish_gizmo(&mut editor).unwrap();
    let after = records(&editor);
    assert_eq!(after["e_gesturea1"], current["e_gesturea1"]);
    assert!(
        after
            .as_object()
            .unwrap()
            .values()
            .any(|record| record["name"] == "Renamed during gesture copy")
    );
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert!(editor.gesture.is_none());
}

#[test]
fn deleted_source_refuses_alt_duplicate_without_partial_copies() {
    let (mut editor, mut shell) = open();
    let mut controller = preview(&mut editor, &mut shell);
    editor
        .apply(
            vec![json!({"cmd":"entity.delete","args":{"entities":["e_gesturea1"]}})],
            "Delete a source during preview",
            None,
        )
        .unwrap();
    let current = records(&editor);
    let revision = editor.host.store().revision();
    assert!(controller.finish_gizmo(&mut editor).is_err());
    assert_eq!(records(&editor), current);
    assert_eq!(editor.host.store().revision(), revision);
    assert!(editor.gesture.is_none());
}

#[test]
fn pointer_cancellation_discards_alt_preview_and_all_native_capture_before_release() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    let mut controller = preview(&mut editor, &mut shell);
    controller.buttons[1] = true;
    controller.keys.insert(226);
    let menu = shell
        .shown_node(shell.menu_header_node("File").unwrap())
        .unwrap();
    let rect = shell.ui.nodes[menu].rect;
    let mut clipboard = crate::native::SessionClipboard::new(true);
    shell
        .event(
            InputEvent::PointerDown([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]),
            &mut clipboard,
        )
        .unwrap();
    assert!(shell.input.captured().is_some());
    let focus = shell.input.focused();
    assert!(editor.gesture.is_some());

    // The same cancellation entry point is used by the viewport key owner and
    // the outer drag router, which can consume Escape before Shell::key.
    controller
        .cancel_pointer_gestures(&mut editor, &mut shell)
        .unwrap();
    assert!(editor.gesture.is_none());
    assert!(controller.gizmo.is_none());
    assert!(!controller.buttons_down());
    assert!(controller.keys.is_empty());
    assert!(shell.input.captured().is_none());
    assert_eq!(shell.input.focused(), focus);
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
    // A physical release arriving after Escape cannot commit the old preview.
    controller.finish_gizmo(&mut editor).unwrap();
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn alt_pointer_chord_does_not_turn_the_next_escape_into_a_menu_mnemonic_cancel() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    let mut clipboard = crate::native::SessionClipboard::new(true);
    for (case, chord) in [
        ("retained-press", true),
        ("retained-wheel", true),
        ("native-wheel", true),
        ("native-held-motion", true),
        ("retained-held-motion", true),
        ("hover", false),
    ] {
        let viewport = shell.viewport().unwrap();
        let point = [viewport.x + viewport.w * 0.5, viewport.y + viewport.h * 0.5];
        let mut native_owner = Controller::default();
        if case == "native-held-motion" {
            native_owner.buttons[1] = true;
        } else if case == "retained-held-motion" {
            let menu = shell
                .shown_node(shell.menu_header_node("File").unwrap())
                .unwrap();
            let rect = shell.ui.nodes[menu].rect;
            shell
                .event(
                    InputEvent::PointerDown([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]),
                    &mut clipboard,
                )
                .unwrap();
            assert!(shell.input.captured().is_some());
        }
        shell.key(&mut editor, 226, true, &mut clipboard).unwrap();
        match case {
            "retained-press" => shell
                .event(InputEvent::PointerDown(point), &mut clipboard)
                .unwrap(),
            "retained-wheel" => shell
                .event(
                    InputEvent::Wheel {
                        position: point,
                        delta: [0., 28.],
                    },
                    &mut clipboard,
                )
                .unwrap(),
            "native-wheel" => native_owner.note_pointer_chord(&Event::Wheel([0., 1.]), &mut shell),
            "native-held-motion" => native_owner.note_pointer_chord(
                &Event::MouseMotion {
                    position: point,
                    delta: [1., 0.],
                },
                &mut shell,
            ),
            "retained-held-motion" => shell
                .event(InputEvent::PointerMove(point), &mut clipboard)
                .unwrap(),
            "hover" => {
                // Neither native nor retained hover is use of an Alt chord.
                native_owner.note_pointer_chord(
                    &Event::MouseMotion {
                        position: point,
                        delta: [1., 0.],
                    },
                    &mut shell,
                );
                shell
                    .event(InputEvent::PointerMove(point), &mut clipboard)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        shell.cancel_pointer_capture();
        shell.key(&mut editor, 226, false, &mut clipboard).unwrap();
        let mut controller = preview(&mut editor, &mut shell);
        shell.key(&mut editor, 41, true, &mut clipboard).unwrap();
        assert_eq!(
            editor.gesture.is_none(),
            chord,
            "{case}: pointer use consumes Alt tap; hover preserves genuine menu activation"
        );
        controller
            .cancel_pointer_gestures(&mut editor, &mut shell)
            .unwrap();
        assert_eq!(records(&editor), before, "{case}");
        assert_eq!(editor.host.store().revision(), revision, "{case}");
    }
}
