//! Full-log reachability and stable reading positions use real command transactions.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const ENTITY: &str = "e_history001";
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target().join("history-panel").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"History panel","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_history01","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        editor.apply(vec![json!({"cmd":"entity.create","args":{"id":ENTITY,"name":"History fixture","components":{"Transform":{}}}})],"History oldest-only marker",None).unwrap();
        for index in 0..140 {
            editor.apply(vec![json!({"cmd":"property.set","args":{"entity":ENTITY,"path":"name","value":format!("History {index}")}})],&format!("History rename {index}"),None).unwrap();
        }
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"history","slot":"view"}),
            )
            .unwrap();
        editor
            .action("editor.focus", json!({"panel":"history"}))
            .unwrap();
        editor
            .action(
                "editor.search",
                json!({"panel":"history","query":"history"}),
            )
            .unwrap();
        let shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn older_pages_and_search_reach_beyond_latest128_and_keep_background_reading_position() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let revision = editor.host.store().revision();
    let camera = editor.state.camera;
    let selection = editor.state.selection.clone();
    let first = projection::page(editor.host.store(), &shell.history_view, "history");
    assert_eq!(first.rows.len(), PAGE);
    assert!(first.older);
    shell.page_history(&mut editor, true).unwrap();
    shell.page_history(&mut editor, true).unwrap();
    let page = projection::page(editor.host.store(), &shell.history_view, "history");
    assert_eq!(page.start, 128);
    assert_eq!(page.rows.len(), 13);
    assert!(!page.older);
    let anchor = page.rows[0].id.clone();
    let oldest = page.rows.last().unwrap().id.clone();
    shell.inspect_history(&mut editor, &oldest.0).unwrap();
    assert_eq!(editor.host.store().revision(), revision);
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":ENTITY,"path":"name","value":"Background rename"}})],"Background content change",None).unwrap();
    shell.rebuild(&mut editor).unwrap();
    let page = projection::page(editor.host.store(), &shell.history_view, "history");
    assert_eq!(page.rows[0].id, anchor);
    assert_eq!(shell.history_view.selected.as_ref(), Some(&oldest));
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    editor
        .action(
            "editor.search",
            json!({"panel":"history","query":"oldest-only marker"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(
        projection::page(
            editor.host.store(),
            &shell.history_view,
            "oldest-only marker"
        )
        .total,
        1
    );
    assert!(shell.shown_node(&format!("history-row-{oldest}")).is_some());
    assert!(shell.shown_node("history-detail").is_some());
}

#[test]
fn full_copy_keeps_real_ops_and_stale_transaction_actions_never_author() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let txn = editor.host.store().history().last().unwrap().clone();
    let revision = editor.host.store().revision();
    shell.inspect_history(&mut editor, &txn.id.0).unwrap();
    shell.copy_history_transaction(&editor, &txn.id.0).unwrap();
    let copied: Value = ara_json::parse(&shell.take_clipboard().unwrap()).unwrap();
    assert_eq!(copied, serde_json::to_value(&txn).unwrap());
    assert!(shell.inspect_history(&mut editor, "tx_missing").is_err());
    assert!(
        shell
            .copy_history_transaction(&editor, "tx_missing")
            .is_err()
    );
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(shell.history_view.selected.as_ref(), Some(&txn.id));
}

#[test]
fn menu_and_history_controls_keep_the_original_action_title_across_repeated_undo_redo() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let title = "undo history action";
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":ENTITY,"path":"name","value":"Repeated history"}})], title, None).unwrap();
    let original = editor.host.store().history().last().unwrap().clone();
    for _ in 0..3 {
        for (command, verb) in [("edit.undo", "Undo"), ("edit.redo", "Redo")] {
            shell.rebuild(&mut editor).unwrap();
            shell.frame().unwrap();
            let target = commands::Target::None;
            let context = shell.ctx(&editor, &target);
            let spec = commands::all()
                .iter()
                .find(|spec| spec.id == command)
                .unwrap();
            assert_eq!(
                spec.title(&context),
                format!("{verb} \u{201c}{title}\u{201d}")
            );
            let node = shell
                .shown_node(if command == "edit.undo" {
                    "history-undo"
                } else {
                    "history-redo"
                })
                .unwrap();
            assert_eq!(
                shell.ui.nodes[node]
                    .element
                    .attrs
                    .get("description")
                    .map(String::as_str),
                Some(format!("{verb} your latest change: {title}").as_str())
            );
            shell.run_command(&mut editor, command, &target).unwrap();
        }
    }
    assert_eq!(editor.host.store().txn(&original.id), Some(&original));
}
