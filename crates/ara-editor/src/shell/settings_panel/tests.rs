//! Extension shortcuts use the same retained controls and typed preference history.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::extensions::{Catalog, Command, Menu};
use std::sync::atomic::{AtomicU64, Ordering};

struct Project(std::path::PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = crate::test_target()
            .join("extension-shortcuts")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Shortcut fixture","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_shortcuts","records":{"e_shortcuts":{"name":"Fixture","Transform":{}}}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open(&self.0).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(crate::test_target().join("extension-shortcuts"))
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
fn catalog(ids: &[(&str, &str)]) -> Catalog {
    Catalog {
        generation: Some("fixture-generation".into()),
        commands: ids
            .iter()
            .map(|(id, title)| Command {
                id: (*id).into(),
                title: (*title).into(),
                summary: "Fixture extension".into(),
                enabled: true,
                disabled_reason: None,
            })
            .collect(),
        menus: ids
            .iter()
            .map(|(id, title)| Menu {
                path: format!("Tools/{title}"),
                command: (*id).into(),
            })
            .collect(),
    }
}
fn set_shortcuts(editor: &mut Editor, value: Value) {
    editor
        .action(
            "editor.settings.set",
            json!({"key":"input.shortcuts","value":value}),
        )
        .unwrap();
}
fn show_shortcuts(editor: &mut Editor, shell: &mut Shell) {
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"settings","slot":"view"}),
        )
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"settings"}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"settings"}))
        .unwrap();
    editor
        .action(
            "editor.settings.view",
            json!({"scope":"user","category":"Keyboard shortcuts"}),
        )
        .unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"shortcuts","query":"Fixture"}),
        )
        .unwrap();
    shell.rebuild(editor).unwrap();
    shell.settle_transitions();
    shell.frame().unwrap();
}

#[test]
fn saved_override_activates_when_async_catalog_arrives_and_retires_when_removed() {
    let project = Project::new();
    let mut editor = project.open();
    set_shortcuts(&mut editor, json!({"Fixture.Command":"F11"}));
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    assert!(shell.keymap.chords("Fixture.Command").is_empty());
    let revision = shell.keymap_revision;
    editor.extensions = catalog(&[("Fixture.Command", "Fixture Action")]);
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.keymap.chords("Fixture.Command")[0].text(), "F11");
    assert!(shell.keymap_revision > revision);
    // Even a source that omits/churns no generation still changes the inventory.
    editor.extensions.commands.clear();
    shell.recording = Some("Fixture.Command".into());
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.keymap.chords("Fixture.Command").is_empty());
    assert!(shell.recording.is_none());
    assert_eq!(
        editor.state.settings["input.shortcuts"]["Fixture.Command"],
        "F11"
    );
}

#[test]
fn extension_rebind_and_reset_use_retained_buttons_and_typed_preferences() {
    let project = Project::new();
    let mut editor = project.open();
    editor.extensions = catalog(&[("Fixture.Command", "Fixture Action")]);
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    show_shortcuts(&mut editor, &mut shell);
    let mut clip = Clip;
    let id = shortcut_row_id("Fixture.Command", false);
    shell.click(&format!("rebind-{id}"), &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(shell.recording.as_deref(), Some("Fixture.Command"));
    shell.key(&mut editor, 68, true, &mut clip).unwrap();
    shell.key(&mut editor, 68, false, &mut clip).unwrap();
    assert_eq!(
        editor.state.settings["input.shortcuts"]["Fixture.Command"],
        "F11"
    );
    assert_eq!(shell.keymap.chords("Fixture.Command")[0].text(), "F11");
    shell.frame().unwrap();
    shell.click(&format!("unbind-{id}"), &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(
        editor.state.settings["input.shortcuts"]
            .get("Fixture.Command")
            .is_none()
    );
    assert!(shell.keymap.chords("Fixture.Command").is_empty());
}

#[test]
fn extension_rows_keep_distinct_ids_and_resolve_conflict_titles() {
    let project = Project::new();
    let mut editor = project.open();
    editor.extensions = catalog(&[
        ("Fixture.Command", "Fixture First"),
        ("Fixture-Command", "Fixture Second"),
    ]);
    set_shortcuts(
        &mut editor,
        json!({"Fixture.Command":"F11","Fixture-Command":"F11"}),
    );
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    show_shortcuts(&mut editor, &mut shell);
    let first = shortcut_row_id("Fixture.Command", false);
    let second = shortcut_row_id("Fixture-Command", false);
    assert_ne!(first, second);
    for id in [&first, &second] {
        assert!(shell.ui.find(&format!("rebind-{id}")).is_some());
    }
    let conflict = shell
        .ui
        .find(&format!("shortcut-conflict-{first}"))
        .unwrap();
    assert_eq!(
        shell.ui.nodes[conflict]
            .element
            .attrs
            .get("text")
            .map(String::as_str),
        Some("Also Fixture Second")
    );
}
