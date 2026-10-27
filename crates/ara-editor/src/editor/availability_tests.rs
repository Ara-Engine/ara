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

fn snapshot(editor: &Editor) -> Vec<(String, String)> {
    editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, document)| {
            (
                path.to_owned(),
                ara_doc::write(document, editor.host.store().schema()),
            )
        })
        .collect()
}

#[test]
fn resource_creation_is_one_attributed_transaction_in_every_domain() {
    let project = Project::new();
    let mut editor = project.open();
    for domain in ["scene", "prefab", "material", "vfx", "graph", "animation"] {
        let extension = match domain {
            "scene" => "scene",
            "prefab" => "prefab",
            "animation" => "animation",
            _ => "graph",
        };
        let path = format!("{domain}-atomic.{extension}");
        let before = snapshot(&editor);
        let revision = editor.host.store().revision();
        let history = editor.host.store().history().len();
        let created = editor
            .action(
                "editor.resource.create",
                json!({"path":path,"domain":domain}),
            )
            .unwrap();
        let doc = created["doc"].as_str().unwrap();
        assert_eq!(created["path"], path);
        assert_eq!(editor.host.store().revision(), revision + 1, "{domain}");
        assert_eq!(editor.host.store().history().len(), history + 1, "{domain}");
        let txn = editor.host.store().history().last().unwrap();
        assert_eq!(txn.author, ara_ops::Author::Human);
        assert!(txn.ops.iter().all(|op| op.doc().as_str() == doc));
        assert!(matches!(
            txn.ops.first(),
            Some(ara_doc::Op::Document { .. })
        ));
        let document = editor.host.store().workspace().doc(doc).unwrap();
        if domain == "animation" {
            crate::authoring::animation::controller(document).unwrap();
        } else if matches!(domain, "material" | "vfx" | "graph") {
            let graph = ara_graph::Graph::from_document(document).unwrap();
            assert_eq!(graph.header.domain, domain);
            if domain == "vfx" {
                ara_vfx::Effect::from_graph(&graph).unwrap();
            }
        }
        let after = snapshot(&editor);
        editor.action("history.undo", json!({})).unwrap();
        assert!(
            editor.host.store().workspace().doc(doc).is_none(),
            "{domain}"
        );
        assert!(
            editor.host.store().workspace().find(&path).is_none(),
            "{domain}"
        );
        assert_eq!(snapshot(&editor), before, "{domain}");
        editor.action("history.redo", json!({})).unwrap();
        assert_eq!(snapshot(&editor), after, "{domain}");
        assert!(editor.host.store().workspace().doc(doc).is_some());
    }
}

#[test]
fn failed_resource_initialization_or_later_batch_command_leaves_no_empty_document() {
    let project = Project::new();
    let mut editor = project.open();
    let before = snapshot(&editor);
    let revision = editor.host.store().revision();
    let history = editor.host.store().history().len();
    for (path, domain, source) in [
        ("invalid-material.graph", "material", "not material source"),
        ("invalid-vfx.graph", "vfx", "not effect source"),
        ("invalid-domain.graph", "unknown", ""),
    ] {
        assert!(
            editor
                .action(
                    "editor.resource.create",
                    json!({"path":path,"domain":domain,"source":source})
                )
                .is_err()
        );
        assert!(editor.host.store().workspace().find(path).is_none());
        assert_eq!(snapshot(&editor), before);
        assert_eq!(editor.host.store().revision(), revision);
        assert_eq!(editor.host.store().history().len(), history);
    }
    // This fails after a complete initializer was staged, not only in source parsing.
    assert!(editor.apply(vec![
        json!({"cmd":"editor.resource.create.atomic","args":{"path":"rolled-back.animation","domain":"animation"}}),
        json!({"cmd":"entity.delete","args":{"entities":["e_absentrecord"]}}),
    ], "Fail the resource batch", None).is_err());
    assert!(
        editor
            .host
            .store()
            .workspace()
            .find("rolled-back.animation")
            .is_none()
    );
    assert_eq!(snapshot(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(editor.host.store().history().len(), history);
}

#[test]
fn old_history_with_missing_animation_initializer_recovers_on_redo_without_quarantine() {
    let project = Project::new();
    let mut editor = project.open();
    // Reproduce the historical service's two real typed transactions exactly.
    let created = editor
        .apply(
            vec![json!({"cmd":"doc.create","args":{"path":"old.animation","kind":"animation"}})],
            "Legacy resource create",
            None,
        )
        .unwrap();
    let doc = created["outputs"][0]["id"].as_str().unwrap().to_owned();
    editor.apply_to(&doc, vec![json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":{}}})], "Legacy resource initialization").unwrap();
    editor
        .action("editor.workspace", json!({"resource":doc,"asset":""}))
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"animation"}))
        .unwrap();
    // Tool pages initially open on a floating surface. This fixture inspects the
    // main Ui, so dock the real page there through the production layout command.
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"animation","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("animation-canvas").is_some());
    let camera = editor.state.camera;
    let resource = editor.state.resource.clone();
    let valid = snapshot(&editor);
    assert!(editor.apply_to(&doc, vec![json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":{"states":1}}})], "Reject malformed controller").is_err());
    assert_eq!(snapshot(&editor), valid);
    // Undo removes only the initializer in that legacy history, preserving its document.
    editor.action("history.undo", json!({})).unwrap();
    assert!(editor.host.store().workspace().doc(&doc).is_some());
    assert!(
        crate::authoring::animation::controller(editor.host.store().workspace().doc(&doc).unwrap())
            .is_err()
    );
    for _ in 0..3 {
        present(&mut editor, &mut shell);
        assert!(shell.ui.find("animation-unavailable").is_some());
        assert!(shell.ui.find("animation-inspector-unavailable").is_some());
        assert!(shell.ui.find("animation-add-state").is_none());
        assert_eq!(editor.state.resource, resource);
        assert_eq!(editor.state.camera, camera);
    }
    editor.action("history.redo", json!({})).unwrap();
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("animation-canvas").is_some());
    assert!(shell.ui.find("animation-unavailable").is_none());
    editor.action("history.undo", json!({})).unwrap();
    present(&mut editor, &mut shell);
    assert!(shell.ui.find("animation-unavailable").is_some());
    editor.action("history.redo", json!({})).unwrap();
    present(&mut editor, &mut shell);
    assert_eq!(editor.state.resource, resource);
    assert_eq!(editor.state.camera, camera);
}
