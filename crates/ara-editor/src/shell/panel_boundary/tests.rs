//! Exercise the production boundary with a real shell and project authority.
use super::*;
use crate::shell::{Binding, commands::Target};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("panel-boundary-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Panel recovery","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_boundary1","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn panic_quarantines_only_its_panel_and_discards_partial_bindings() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
    let prior = shell.bindings.len();
    let mut markup = String::new();
    shell.with_panel_boundary("inspector", &mut editor, &mut markup, |shell, _, out| {
        out.push_str("<panel id='unfinished'>");
        shell
            .bindings
            .insert(123, Binding::Command("file.quit", Target::None));
        panic!("injected inspector failure <&>");
    });
    assert_eq!(shell.bindings.len(), prior + 1);
    assert!(!shell.bindings.contains_key(&123));
    assert!(!markup.contains("unfinished"));
    assert!(markup.contains("panel-error-inspector"));
    assert!(markup.contains("&lt;&amp;&gt;"));

    // A quarantined panel must not panic or report again on every frame.
    shell.with_panel_boundary("inspector", &mut editor, &mut markup, |_, _, _| {
        panic!("quarantined renderer was called again");
    });
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.ui.find("body-hierarchy").is_some());
    let logs = editor
        .action("logs", json!({"codes":["ARA4500"],"group":true,"limit":20}))
        .unwrap();
    assert_eq!(logs["summary"]["occurrences"], 1);
    assert!(shell.retry_panel("inspector"));
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.ui.find("panel-error-inspector").is_none());
}

#[test]
fn returned_panel_error_uses_the_same_recovery_path() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
    let mut markup = String::new();
    shell.with_panel_boundary("assets", &mut editor, &mut markup, |_, _, _| {
        Err("missing imported resource".into())
    });
    assert!(markup.contains("missing imported resource"));
    assert!(!shell.panel_failures["assets"].panicked);
    shell.with_panel_boundary("console", &mut editor, &mut markup, |_, _, out| {
        out.push_str("<text id='other-panel' text='still works' />");
        Ok(())
    });
    assert!(markup.contains("still works"));
}
