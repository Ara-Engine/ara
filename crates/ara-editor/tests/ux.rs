#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! Editor UX v2: the command registry walk, menus, detach/redock, settings undo
//! and empty states, through the real shell and protocol.
use ara_editor::{Editor, shell::Shell, world::EditWorldView};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ara-editor-ux-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"UX test","main":"levels/main.scene"}),
        );
        write(
            &root.join("levels/main.scene"),
            json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{"e_crate001":{"name":"Salvage crate","Transform":{"position":[0,0.6,0]},"MeshRenderer":{}},"e_child001":{"name":"Child","parent":"e_crate001","Transform":{"position":[1,0,0]}}}}),
        );
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, value: Value) {
    std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
}
#[derive(Default)]
struct MemoryClipboard(String);
impl ara_ui_text::Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

#[test]
fn every_menu_context_menu_and_toolbar_button_runs_a_registered_command() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The registry export (MCP) lists every command with a human title and menu path.
    let exported = editor.action("editor.commands", json!({})).unwrap();
    let commands = exported["commands"].as_array().unwrap();
    assert!(commands.len() > 150, "{} commands", commands.len());
    let ids: std::collections::BTreeSet<_> =
        commands.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), commands.len(), "command ids are unique");
    for c in commands {
        assert!(!c["title"].as_str().unwrap().is_empty());
        if c["enabled"] == false {
            assert!(
                c["disabledReason"].as_str().is_some_and(|r| !r.is_empty()),
                "{c}"
            );
        }
    }
    let duplicate = commands
        .iter()
        .find(|c| c["id"] == "edit.duplicate")
        .unwrap();
    assert_eq!(duplicate["menu"], "Edit");
    assert_eq!(duplicate["shortcuts"][0], "Ctrl+D");
    // Every menu (and every submenu) row is bound to a registered command.
    for walk in shell.walk_menus(&mut editor).unwrap() {
        assert!(ids.contains(walk.1.as_str()), "{} -> {}", walk.0, walk.1);
    }
    // Chrome and panel toolbars: no button without a registry command.
    let bound = shell.command_bindings();
    assert!(
        bound
            .iter()
            .any(|(id, c)| id == "play" && *c == "play.start")
    );
    assert!(
        bound
            .iter()
            .any(|(id, c)| id == "search-box" && *c == "edit.palette")
    );
    assert!(
        bound
            .iter()
            .any(|(id, c)| id == "tool-Translate" && *c == "view.tool.move")
    );
    let loose = shell.uncommanded_buttons(&[
        "tool-",
        "snap-",
        "play",
        "pause",
        "step",
        "stop",
        "search-box",
        "chip-",
        "status-",
        "panel-menu-",
        "menu-title-",
        "frame",
    ]);
    assert!(
        loose.is_empty(),
        "toolbar buttons without commands: {loose:?}"
    );
}

#[test]
fn menus_label_undo_explain_disabled_items_and_filter_as_you_type() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // Nothing to undo yet: the item is disabled and says why.
    let rows = shell.menu_rows(&mut editor, 1).unwrap();
    let undo = rows.iter().find(|r| r.0 == "edit.undo").unwrap();
    assert_eq!(undo.1, "Undo");
    assert_eq!(undo.2.as_deref(), Some("Nothing to undo"));
    editor
        .apply(vec![json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"Transform.position.x","value":2}})], "Move crate", None)
        .unwrap();
    let rows = shell.menu_rows(&mut editor, 1).unwrap();
    let undo = rows.iter().find(|r| r.0 == "edit.undo").unwrap();
    assert_eq!(undo.1, "Undo \u{201c}Move crate\u{201d}");
    assert!(undo.2.is_none());
    // Type-to-filter flattens submenus: "grid" finds Snapping ▸ Grid Snap Size….
    let found = shell.filter_menu(&mut editor, 1, "grid").unwrap();
    assert!(found.iter().any(|r| r.0 == "snap.grid"), "{found:?}");
    // The keymap runs Edit ▸ Undo through Ctrl+Z.
    let mut clipboard = MemoryClipboard::default();
    shell.key(&mut editor, 224, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 29, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 224, false, &mut clipboard).unwrap();
    let x =
        editor.view().unwrap().entities()["e_crate001"].record["Transform"]["position"][0].clone();
    assert_eq!(x, json!(0));
}

#[test]
fn panels_detach_into_floating_windows_and_redock() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"inspector","slot":"float","rect":[900,120,420,600]}),
        )
        .unwrap();
    assert_eq!(editor.state.floating.len(), 1);
    let window = editor.state.floating[0].id.clone();
    assert_eq!(editor.state.floating[0].rect, [900, 120, 420, 600]);
    shell.rebuild(&mut editor).unwrap();
    // The main window no longer hosts the Inspector; its floating surface does.
    assert!(shell.ui.find("inspector-scroll").is_none());
    assert!(shell.enter(&window));
    assert!(shell.ui.find("inspector-scroll").is_some());
    assert!(shell.ui.find("edit-Transform-position-0").is_some());
    shell.leave();
    // Window rects persist with the layout and survive a reopen.
    editor
        .action("editor.window", json!({"id":window,"rect":[40,60,500,640]}))
        .unwrap();
    drop(shell);
    drop(editor);
    let mut editor = Editor::open(&project.0).unwrap();
    assert_eq!(editor.state.floating.len(), 1);
    assert_eq!(editor.state.floating[0].rect, [40, 60, 500, 640]);
    // Dragging the tab onto a main-window area docks it back and closes the window.
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.drag_tab_to(&mut editor, "inspector", "view").unwrap();
    assert!(editor.state.floating.is_empty());
    assert!(ara_editor::state::panel_visible(&editor.state, "inspector"));
    // The Scene viewport stays in the main window.
    assert!(
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"scene","slot":"float"})
            )
            .is_err()
    );
    // Built-in layouts apply and Save as round-trips through .ara/layouts.
    editor
        .action("editor.layout.apply", json!({"name":"levelDesign"}))
        .unwrap();
    editor
        .action("editor.layout.save", json!({"name":"My layout"}))
        .unwrap();
    editor.action("editor.layout.reset", json!({})).unwrap();
    editor
        .action("editor.layout.apply", json!({"name":"My layout"}))
        .unwrap();
    assert_eq!(
        editor.state.layout,
        ara_editor::state::preset_layout("levelDesign").unwrap()
    );
}

#[test]
fn settings_edits_are_undoable_commands_in_both_scopes() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell
        .run_command(&mut editor, "edit.preferences", &Default::default())
        .unwrap();
    assert!(ara_editor::state::panel_visible(&editor.state, "settings"));
    editor
        .action("editor.focus", json!({"panel":"settings"}))
        .unwrap();
    editor
        .action(
            "editor.settings.set",
            json!({"key":"appearance.theme","value":"light"}),
        )
        .unwrap();
    assert!(editor.state.light);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.tickHz","value":"120"}),
        )
        .unwrap();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(project.0.join("ara.project.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["settings"]["play.tickHz"], "120");
    assert_eq!(manifest["main"], "levels/main.scene");
    // Edit ▸ Undo follows focus: in Settings it undoes the last setting, with its label.
    let rows = shell.menu_rows(&mut editor, 1).unwrap();
    assert_eq!(
        rows.iter().find(|r| r.0 == "edit.undo").unwrap().1,
        "Undo \u{201c}Set Fixed tick rate\u{201d}"
    );
    shell
        .run_command(&mut editor, "edit.undo", &Default::default())
        .unwrap();
    shell
        .run_command(&mut editor, "edit.undo", &Default::default())
        .unwrap();
    assert!(!editor.state.light);
    assert_eq!(editor.state.settings["play.tickHz"], "60");
    shell
        .run_command(&mut editor, "edit.redo", &Default::default())
        .unwrap();
    assert!(editor.state.light);
    // Invalid values are rejected with a reason; nothing changes.
    let error = editor
        .action(
            "editor.settings.set",
            json!({"key":"appearance.density","value":"huge"}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Density"), "{error}");
    // A custom shortcut overrides the preset and shows up in the keymap export.
    editor
        .action(
            "editor.settings.set",
            json!({"key":"input.shortcuts","value":{"edit.duplicate":"Ctrl+J"}}),
        )
        .unwrap();
    let exported = editor.action("editor.commands", json!({})).unwrap();
    let duplicate = exported["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "edit.duplicate")
        .unwrap()
        .clone();
    assert_eq!(duplicate["shortcuts"][0], "Ctrl+J");
    editor
        .action(
            "editor.settings.set",
            json!({"key":"input.keymap","value":"unity"}),
        )
        .unwrap();
    let exported = editor.action("editor.commands", json!({})).unwrap();
    let play = exported["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "play.start")
        .unwrap()
        .clone();
    assert_eq!(play["shortcuts"][0], "Ctrl+P");
}

#[test]
fn panels_show_real_empty_states() {
    let project = Project::new();
    std::fs::write(
        project.0.join("levels/main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{}}),
        ),
    )
    .unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    // The console filters to nothing; project diagnostics may exist.
    editor
        .action(
            "editor.search",
            json!({"panel":"console","query":"no-such-message"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    for (panel, empty) in [
        ("hierarchy", "hierarchy-empty"),
        ("inspector", "inspector-empty"),
        ("game", "game-empty"),
        ("console", "console-empty"),
        ("agents", "agents-empty"),
        ("profiler", "profile-empty"),
        ("vfx", "vfx-empty"),
        ("material", "material-empty"),
        ("animation", "animation-empty"),
    ] {
        editor
            .action("editor.focus", json!({"panel":panel}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        let index = shell
            .ui
            .find(empty)
            .unwrap_or_else(|| panic!("{panel} has no empty state"));
        assert!(shell.ui.nodes[index].visible);
        // Every empty state has a headline and a hint.
        assert!(
            shell.ui.find(&format!("{empty}-title")).is_some(),
            "{panel}"
        );
        assert!(shell.ui.find(&format!("{empty}-hint")).is_some(), "{panel}");
    }
    // The hierarchy's empty state offers Create, bound to the registry.
    let bound = shell.command_bindings();
    editor
        .action("editor.focus", json!({"panel":"hierarchy"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let bound_after = shell.command_bindings();
    assert!(
        bound_after
            .iter()
            .chain(bound.iter())
            .any(|(id, c)| id == "hierarchy-empty-create" && *c == "create.empty")
    );
}

#[test]
fn context_menus_open_for_rows_fields_tabs_and_empty_space() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    for target in [
        "hierarchy-row",
        "field-Transform-position",
        "tab-scene",
        "hierarchy-rows",
    ] {
        let rows = shell.context_rows_at(&mut editor, target).unwrap();
        assert!(!rows.is_empty(), "no context menu on {target}");
    }
    let rows = shell.context_rows_at(&mut editor, "hierarchy-row").unwrap();
    assert!(
        rows.iter().any(|r| r == "edit.rename") && rows.iter().any(|r| r == "edit.delete"),
        "{rows:?}"
    );
    let rows = shell
        .context_rows_at(&mut editor, "field-Transform-position")
        .unwrap();
    assert!(
        rows.iter().any(|r| r == "field.reset") && rows.iter().any(|r| r == "field.copy_path"),
        "{rows:?}"
    );
    let rows = shell.context_rows_at(&mut editor, "tab-scene").unwrap();
    assert!(
        rows.iter().any(|r| r == "window.float") && rows.iter().any(|r| r == "window.maximize"),
        "{rows:?}"
    );
}
