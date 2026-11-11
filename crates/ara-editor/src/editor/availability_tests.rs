//! Missing authoring sources are view states, not panel or application failures.
#![allow(clippy::unwrap_used)]
use super::*;
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
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Scene recovery","main":"levels/main.scene"}),
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

