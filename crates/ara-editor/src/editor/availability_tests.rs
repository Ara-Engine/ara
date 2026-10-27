//! Missing authoring sources are view states, not panel or application failures.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::{
    shell::{Shell, commands::Target},
    world::{EditWorldView, SceneResolver},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target()
            .join("scene-availability")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        assert!(!root.exists());
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                PathBuf::from("ara.project.json"),
                json!({"layoutVersion":3,"name":"Scene recovery","main":"levels/main.scene"}),
            ),
            (
                PathBuf::from(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
                json!({"$schema":"ara://schema/scene/1","id":"sc_available1","records":{"e_available1":{"name":"Restorable","Transform":{},"MeshRenderer":{}}}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(crate::test_target().join("scene-availability"))
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn present(editor: &mut Editor, shell: &mut Shell) {
    let revision = editor.host.store().revision();
    shell.rebuild(editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.panel_failure_count(), 0);
    let view = shell.document_view(editor).unwrap();
    let root = editor.host.root().to_owned();
    view.scene(
        &root,
        &editor.state,
        editor.gesture.as_ref(),
        &mut editor.models,
    )
    .unwrap();
    editor.view().unwrap();
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Presentation must not author fallback content"
    );
}

#[test]
fn undo_open_scene_creation_preserves_navigation_and_redo_recovers_without_panel_retry() {
    let project = Project::new();
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
    let original = editor.state.doc.clone();
    // The same registry path as native New Scene, including its separate open action.
    for _ in 0..3 {
        shell
            .run_command(&mut editor, "file.new_scene", &Target::None)
            .unwrap();
        present(&mut editor, &mut shell);
    }
    let created = editor.state.doc.clone();
    assert_ne!(created, original);
    editor
        .action(
            "editor.camera",
            json!({"yaw":0.92,"pitch":0.23,"distance":17.0}),
        )
        .unwrap();
    let camera = editor.state.camera;
    let selection = editor.state.selection.clone();
    let before = editor.host.store().revision();
    shell
        .run_command(&mut editor, "edit.undo", &Target::None)
        .unwrap();
    assert_eq!(editor.host.store().revision(), before + 1);
    assert!(!editor.document_available());
    assert_eq!(editor.state.doc, created);
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    assert!(crate::world::resolve(editor.host.store(), &created).is_err());
    assert!(DocumentView::new(editor.host.store(), &created).is_err());
    let mut resolver = SceneResolver::default();
    assert!(resolver.resolve(editor.host.store(), &created).is_err());
    assert!(
        resolver
            .presentation(editor.host.store(), &created)
            .unwrap()
            .is_empty()
    );
    let revision = editor.host.store().revision();
    assert!(
        editor
            .apply(
                vec![json!({"cmd":"entity.create","args":{"name":"Must reject"}})],
                "Missing document",
                None
            )
            .is_err()
    );
    assert_eq!(editor.host.store().revision(), revision);
    for _ in 0..3 {
        present(&mut editor, &mut shell);
        assert!(shell.ui.find("hierarchy-unavailable").is_some());
        assert!(shell.ui.find("inspector-unavailable").is_some());
        assert!(shell.viewport().is_some());
    }
    shell
        .run_command(&mut editor, "edit.redo", &Target::None)
        .unwrap();
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert!(editor.document_available());
    assert_eq!(editor.state.doc, created);
    assert_eq!(editor.state.camera, camera);
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("hierarchy-unavailable").is_none());
    assert!(shell.ui.find("inspector-unavailable").is_none());
    assert!(resolver.resolve(editor.host.store(), &created).is_ok());
}

#[test]
fn deleted_selection_recovers_without_stealing_the_human_view() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.select", json!({"entities":["e_available1"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
    let doc = editor.state.doc.clone();
    let camera = editor.state.camera;
    let selected = editor.state.selection.clone();
    editor
        .apply(
            vec![json!({"cmd":"entity.delete","args":{"entities":["e_available1"]}})],
            "Remove selection",
            None,
        )
        .unwrap();
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("inspector-selection-unavailable").is_some());
    assert_eq!(editor.state.selection, selected);
    assert_eq!(editor.state.camera, camera);
    editor.action("history.undo", json!({})).unwrap();
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("inspector-selection-unavailable").is_none());
    assert_eq!(editor.state.doc, doc);
    assert_eq!(editor.state.camera, camera);
}
