//! Missing explicit resource identities never redirect edits to another document.
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
        let root = crate::test_target().join("resource-identity").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!root.exists());
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                PathBuf::from("ara.project.json"),
                json!({"layoutVersion":3,"name":"Resource identity","main":"levels/main.scene"}),
            ),
            (
                PathBuf::from(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
                json!({"$schema":"ara://schema/scene/1","id":"sc_resource1","records":{}}),
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
                .starts_with(crate::test_target().join("resource-identity"))
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn create(editor: &mut Editor, name: &str, domain: &str) -> String {
    let extension = if domain == "animation" {
        "animation"
    } else {
        "graph"
    };
    editor
        .action(
            "editor.resource.create",
            json!({"path":format!("{name}.{extension}"),"domain":domain}),
        )
        .unwrap()["doc"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn snapshot(editor: &Editor) -> Vec<(String, String)> {
    editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, doc)| {
            (
                path.to_owned(),
                ara_doc::write(doc, editor.host.store().schema()),
            )
        })
        .collect()
}

#[test]
fn removing_the_second_open_resource_never_retargets_the_first_and_redo_recovers() {
    for domain in ["material", "vfx", "animation"] {
        let project = Project::new();
        let mut editor = project.open();
        let first = create(&mut editor, "first", domain);
        let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
        assert!(editor.state.resource.is_none());
        assert_eq!(
            shell.resource_for(&editor, domain).as_deref(),
            Some(first.as_str()),
            "An unselected page retains its normal initial default"
        );
        let before = snapshot(&editor);
        let second = create(&mut editor, "second", domain);
        editor
            .action("editor.workspace", json!({"resource":second,"asset":""}))
            .unwrap();
        editor
            .action("editor.layout.move", json!({"panel":domain,"slot":"view"}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(
            shell.resource_for(&editor, domain).as_deref(),
            Some(second.as_str())
        );
        assert_eq!(shell.panel_failure_count(), 0);
        let camera = editor.state.camera;
        let opened = snapshot(&editor);
        let module = if domain == "vfx" {
            let graph = ara_graph::Graph::from_document(
                editor.host.store().workspace().doc(&second).unwrap(),
            )
            .unwrap();
            let effect = ara_vfx::Effect::from_graph(&graph).unwrap();
            let emitter = effect.emitters.first().unwrap();
            Some(format!(
                "{}|initialize|{}",
                emitter.name,
                emitter.stacks.initialize.first().unwrap().name
            ))
        } else {
            None
        };
        editor.action("history.undo", json!({})).unwrap();
        let revision = editor.host.store().revision();
        assert!(editor.host.store().workspace().doc(&second).is_none());
        assert_eq!(snapshot(&editor), before);
        for _ in 0..3 {
            shell.rebuild(&mut editor).unwrap();
            shell.frame().unwrap();
            assert_eq!(shell.panel_failure_count(), 0);
            assert_eq!(shell.resource_for(&editor, domain), None);
            assert!(shell.ui.find(&format!("{domain}-empty")).is_some());
            assert_eq!(editor.state.resource.as_deref(), Some(second.as_str()));
            assert_eq!(editor.state.camera, camera);
            assert_eq!(snapshot(&editor), before);
        }
        // These are the production authoring handlers behind the graph menu,
        // emitter menu and Animation toolbar, including retained stale canvases.
        match domain {
            "material" => {
                let _ = shell.edit_graph_layout(&mut editor, "comment");
            }
            "vfx" => assert!(
                shell
                    .edit_vfx_module(&mut editor, module.as_deref().unwrap(), "duplicate")
                    .is_err()
            ),
            "animation" => shell.edit_animation(&mut editor, "state").unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            snapshot(&editor),
            before,
            "{domain} edits must never fall through to the first resource"
        );
        assert_eq!(editor.host.store().revision(), revision);
        assert_eq!(editor.state.resource.as_deref(), Some(second.as_str()));
        assert_eq!(editor.state.camera, camera);
        editor.action("history.redo", json!({})).unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(
            shell.resource_for(&editor, domain).as_deref(),
            Some(second.as_str())
        );
        assert!(shell.ui.find(&format!("{domain}-empty")).is_none());
        assert_eq!(shell.panel_failure_count(), 0);
        assert_eq!(snapshot(&editor), opened);
        assert_eq!(editor.state.camera, camera);
    }
}

#[test]
fn a_valid_selection_of_another_domain_retains_each_pages_existing_default() {
    let project = Project::new();
    let mut editor = project.open();
    let material = create(&mut editor, "paint", "material");
    let vfx = create(&mut editor, "fire", "vfx");
    let animation = create(&mut editor, "walk", "animation");
    editor
        .action("editor.workspace", json!({"resource":material}))
        .unwrap();
    let shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
    assert_eq!(shell.resource_for(&editor, "material"), Some(material));
    assert_eq!(shell.resource_for(&editor, "vfx"), Some(vfx));
    assert_eq!(shell.resource_for(&editor, "animation"), Some(animation));
}
