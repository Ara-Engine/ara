//! Contact math and captured transform commits use the actual authority and gesture path.
#![allow(clippy::unwrap_used)]
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

fn open() -> (Editor, Shell) {
    let root = crate::test_target().join("ux-snap").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("levels")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Snap gesture","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_snap001","records":{}}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    }
    let mut editor = Editor::open_with(&root, Some(root.join(".ara/test-user.json"))).unwrap();
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":"e_snapparent","name":"Scaled parent","components":{"Transform":{"position":[10,2,-3],"rotation":[0,30,0],"scale":[2,3,4]}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_snapsource","name":"Source","parent":"e_snapparent","components":{"Transform":{"position":[1,2,3],"rotation":[0,0,15],"scale":[1,2,1]},"MeshRenderer":{}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_snapchild","name":"Child","parent":"e_snapsource","components":{"Transform":{},"MeshRenderer":{}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_snaptarget","name":"Target","components":{"Transform":{"position":[0,3,0]},"MeshRenderer":{}}}}),
    ],"Create snap fixture",None).unwrap();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_snapsource","e_snapchild"]}),
        )
        .unwrap();
    editor
        .action(
            "editor.tool",
            json!({"tool":"translate","local":true,"snapping":true}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The scene opened empty, before the transformed fixtures were authored.
    // Frame through the real navigation command instead of assuming the opening
    // camera can see a source translated by its scaled/rotated parent.
    let rect = shell.viewport().unwrap();
    let framed = editor
        .action("editor.frame", json!({"aspect":rect.w / rect.h}))
        .unwrap();
    assert_eq!(
        framed["framingPending"], false,
        "Procedural fixture bounds are immediately available"
    );
    shell.rebuild(&mut editor).unwrap();
    let view = editor.view().unwrap();
    let rect = shell.viewport().unwrap();
    let handles = overlay::handles(&view, &editor.state, rect)
        .expect("Frame selected must place the snap fixture in front of its real Scene camera");
    assert!(
        rect.contains(handles.origin[0], handles.origin[1]),
        "Framed source gizmo must be inside the Scene viewport"
    );
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
fn scope(editor: &Editor, shell: &Shell, view: &crate::world::DocumentView) -> Drag {
    // These CPU tests exercise commit guards; rendered vertex acquisition is covered
    // by picking tests and the native SnapCheck on its actual SceneView snapshot.
    Drag {
        doc: editor.state.doc.clone(),
        surface: shell.current().map(str::to_owned),
        roots: editor
            .gesture
            .as_ref()
            .unwrap()
            .initial
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    crate::world::world_transform(view.entities(), id).unwrap(),
                )
            })
            .collect(),
        source_addresses: BTreeSet::from(["e_snapsource".into(), "e_snapchild".into()]),
        excluded: BTreeSet::new(),
        vertices: Vec::new(),
        source_vertex: None,
        target: None,
    }
}

#[test]
fn tilted_surface_contact_keeps_orientation_support_offset_and_bypasses_grid() {
    let normal = glam::Quat::from_rotation_z(0.4) * Vec3::Y;
    let vertices: Vec<_> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { -1. } else { 1. },
                if i & 2 == 0 { -2. } else { 2. },
                if i & 4 == 0 { -0.5 } else { 0.5 },
            ) + Vec3::Y * 8.
        })
        .collect();
    let pivot = Vec3::Y * 8.;
    let point = Vec3::new(4., 3., 2.);
    let gap = 0.125;
    let delta = contact_delta(&vertices, pivot, point, normal, gap).unwrap();
    let nearest = vertices
        .iter()
        .map(|vertex| (*vertex + delta - point).dot(normal))
        .min_by(f32::total_cmp)
        .unwrap();
    assert!((nearest - gap).abs() < 1e-5);
    assert!(
        vertices
            .iter()
            .all(|vertex| (*vertex + delta - point).dot(normal) >= gap - 1e-5)
    );
    assert!(contact_delta(&[], pivot, point, normal, gap).is_none());
    assert!(contact_delta(&vertices, pivot, point, Vec3::ZERO, gap).is_none());
    assert!(contact_delta(&vertices, pivot, point, normal, f32::NAN).is_none());
}

#[test]
fn exact_world_translation_uses_captured_nonuniform_parent_and_commits_one_undo() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    let view = editor.view().unwrap();
    let rect = shell.viewport().unwrap();
    let handles = overlay::handles(&view, &editor.state, rect).unwrap();
    let mut controller = Controller::default();
    controller
        .begin_gizmo(
            &mut editor,
            &handles,
            overlay::Handle::Centre,
            handles.origin,
            rect,
            false,
        )
        .unwrap();
    controller.gizmo.as_mut().unwrap().snap = Some(scope(&editor, &shell, &view));
    let initial = editor.gesture.as_ref().unwrap().initial.clone();
    assert_eq!(initial.len(), 1, "a selected descendant follows its root");
    let parent = crate::world::world_transform(view.entities(), "e_snapparent").unwrap();
    let delta = Vec3::new(0.125, 0.375, -0.625);
    assert!(editor.gesture.as_mut().unwrap().translate_world(delta));
    let preview = editor.gesture.as_ref().unwrap().current.clone();
    let start = parent * initial["e_snapsource"];
    let next = parent * preview["e_snapsource"];
    assert!((next.w_axis.truncate() - start.w_axis.truncate() - delta).length() < 1e-5);
    assert_eq!(
        preview["e_snapsource"].x_axis,
        initial["e_snapsource"].x_axis
    );
    assert_eq!(
        preview["e_snapsource"].y_axis,
        initial["e_snapsource"].y_axis
    );
    assert_eq!(
        preview["e_snapsource"].z_axis,
        initial["e_snapsource"].z_axis
    );
    assert!(
        !editor
            .gesture
            .as_mut()
            .unwrap()
            .translate_world(Vec3::splat(f32::NAN))
    );
    assert_eq!(
        editor.gesture.as_ref().unwrap().current,
        preview,
        "invalid delta is atomic"
    );
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
    controller.finish_snap(&mut editor, &shell).unwrap();
    let after = records(&editor);
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert_eq!(after["e_snapchild"], before["e_snapchild"]);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(records(&editor), before);
    editor.action("history.redo", json!({})).unwrap();
    assert_eq!(records(&editor), after);
    shell.rebuild(&mut editor).unwrap();
}

#[test]
fn deleted_snap_target_or_source_cancels_without_partial_ops() {
    for deleted in ["e_snaptarget", "e_snapsource"] {
        let (mut editor, shell) = open();
        let view = editor.view().unwrap();
        let rect = shell.viewport().unwrap();
        let handles = overlay::handles(&view, &editor.state, rect).unwrap();
        let mut controller = Controller::default();
        controller
            .begin_gizmo(
                &mut editor,
                &handles,
                overlay::Handle::Centre,
                handles.origin,
                rect,
                false,
            )
            .unwrap();
        controller.gizmo.as_mut().unwrap().snap = Some(scope(&editor, &shell, &view));
        let world = crate::world::world_transform(view.entities(), "e_snaptarget").unwrap();
        controller
            .gizmo
            .as_mut()
            .unwrap()
            .snap
            .as_mut()
            .unwrap()
            .target = Some(("e_snaptarget".into(), world));
        assert!(editor.gesture.as_mut().unwrap().translate_world(Vec3::Y));
        editor
            .apply(
                vec![json!({"cmd":"entity.delete","args":{"entities":[deleted]}})],
                "Delete snap participant",
                None,
            )
            .unwrap();
        let before = records(&editor);
        let revision = editor.host.store().revision();
        controller.finish_snap(&mut editor, &shell).unwrap();
        assert_eq!(records(&editor), before);
        assert_eq!(editor.host.store().revision(), revision);
        assert!(editor.gesture.is_none() && controller.gizmo.is_none());
    }
}

#[test]
fn held_vertex_mode_yields_to_primary_shortcuts_fields_and_modal_scopes() {
    let (mut editor, mut shell) = open();
    let mut controller = Controller::default();
    controller.keys.insert(25);
    assert!(controller.vertex_snap(&editor, &shell));
    controller.keys.insert(224);
    assert!(!controller.vertex_snap(&editor, &shell));
    controller.keys.remove(&224);
    for key in [227, 231] {
        controller.keys.insert(key);
        assert!(!controller.vertex_snap(&editor, &shell));
        controller.keys.remove(&key);
    }
    let field = shell
        .shown_node("camera-fov")
        .or_else(|| {
            shell
                .ui
                .nodes
                .iter()
                .enumerate()
                .find(|(i, node)| {
                    shell.ui.shown(*i) && matches!(node.element.tag.as_str(), "field" | "input")
                })
                .map(|(i, _)| i)
        })
        .unwrap();
    let id = ara_ui::source::stable_id(&shell.ui.nodes[field].element.id);
    shell.input.focus(&mut shell.ui, id).unwrap();
    assert!(!controller.vertex_snap(&editor, &shell));
    shell.input.blur(&mut shell.ui);
    shell.dialog = Some(crate::shell::Dialog::about());
    shell.rebuild(&mut editor).unwrap();
    assert!(!controller.vertex_snap(&editor, &shell));
}
