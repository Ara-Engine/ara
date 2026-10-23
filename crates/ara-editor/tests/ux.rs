#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! Editor UX v2: the command registry walk, menus, detach/redock, settings undo
//! and empty states, through the real shell and protocol.
use ara_editor::{Editor, shell::Shell, world::EditWorldView};
use ara_ui::paint::Draw;
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
    // A long reason is cut, and every row (keycaps included) stays inside the menu.
    editor
        .apply(vec![json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"Transform.position.y","value":1}})], "Raise the crate so the verification route has a long reason", None)
        .unwrap();
    let rows = shell.menu_rows(&mut editor, 1).unwrap();
    let undo = rows.iter().find(|r| r.0 == "edit.undo").unwrap();
    assert!(undo.1.ends_with("\u{2026}\u{201d}"), "{}", undo.1);
    let overflow = shell.menu_overflow(&mut editor, 1).unwrap();
    assert!(overflow.is_empty(), "{overflow:?}");
    shell
        .run_command(&mut editor, "edit.undo", &Default::default())
        .unwrap();
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
    // Paint-time decorations (the colour swatch) stay beneath an open menu.
    assert!(
        shell
            .open_context_at(&mut editor, "field-MeshRenderer-color")
            .unwrap()
    );
    let paints = |id: &str| {
        let id = ara_ui::source::stable_id(id);
        let commands = &shell.ui.display.commands;
        (0..commands.len()).filter(move |i| {
            matches!(&commands[*i], Draw::Rect { node, .. } | Draw::RoundedRect { node, .. } if *node == id)
        })
    };
    let swatch = paints("swatch-MeshRenderer-color").next_back().unwrap();
    assert!(swatch < paints("menu-pop-0").next().unwrap());
}
#[test]
fn tinted_icons_take_their_state_color() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The active tool's icon sits on the selection fill, so it uses the selection text color.
    let icon = shell.ui.find("tool-Translate-icon").unwrap();
    assert_eq!(
        Some(shell.ui.nodes[icon].computed.color),
        shell.ui.sheet.color_token("--selection-foreground")
    );
}
#[test]
fn any_scene_opens_with_its_own_camera_and_agents_only_move_their_presence() {
    let project = Project::new();
    write(
        &project.0.join("levels/hangar.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_hangar001","records":{"e_lamp0001":{"name":"Lamp","Transform":{"position":[200,2,190]}}}}),
    );
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // File > Open Scene lists every scene, grouped before assets.
    let rows = shell.palette_rows(&mut editor, "#");
    assert!(
        rows.iter()
            .any(|r| r.0 == "Scenes" && r.1 == "hangar.scene" && r.2 == Some("file.open_scene")),
        "{rows:?}"
    );
    let main_camera = editor.state.camera;
    editor
        .action("editor.scene.open", json!({"scene":"levels/hangar.scene"}))
        .unwrap();
    assert_eq!(editor.state.doc, "sc_hangar001");
    // A first open frames the scene: the lamp is 270 m from the origin.
    assert!((editor.state.camera.target.x - 200.).abs() < 1.);
    assert_eq!(editor.state.recent_scenes[0], "sc_hangar001");
    editor
        .action("editor.scene.open", json!({"scene":"sc_test0001"}))
        .unwrap();
    assert_eq!(editor.state.camera, main_camera);
    assert!(
        editor
            .action("editor.scene.open", json!({"scene":"levels/missing.scene"}))
            .is_err()
    );
    // An agent opening a scene moves its presence, not the human view, and the human
    // gets a View/Follow notice instead.
    let agent = ara_proto::Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"scene-test","author":"agent:scout"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    ara_proto::Endpoint::call(
        &mut editor,
        "editor.scene.open",
        json!({"session":agent,"scene":"levels/hangar.scene"}),
    )
    .unwrap();
    assert_eq!(editor.state.doc, "sc_test0001");
    assert_eq!(editor.state.presence[&agent].doc, "sc_hangar001");
    let notice = editor
        .activity
        .notices()
        .iter()
        .find(|n| n.text == "Scout is viewing hangar.scene")
        .unwrap()
        .id;
    editor.state.focus = "scene".into();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find(&format!("notice-{notice}-follow")).is_some());
    shell
        .run_command(
            &mut editor,
            "agents.view",
            &ara_editor::shell::commands::Target::Text(agent.clone()),
        )
        .unwrap();
    assert_eq!(editor.state.doc, "sc_hangar001");
    assert!(editor.state.follow.is_none());
}
#[test]
fn console_logs_problems_explain_and_editor_failures_reach_the_bus() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    // An editor-side failure is a catalogued diagnostic on the bus (what `ara logs`
    // reads), not a line on stderr.
    editor
        .action(
            "editor.report",
            json!({"message":"Thumbnail failed for crate.glb","kind":"thumbnail","subject":"asset:@models/crate.glb"}),
        )
        .unwrap();
    let logs = editor
        .action("logs", json!({"group": false, "limit": 20}))
        .unwrap();
    assert!(
        logs["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["diagnostic"]["code"] == "ARA4501"),
        "{logs}"
    );
    editor
        .action("editor.layout.show", json!({"panel":"console"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let row = shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.id.ends_with("-text")
                && n.text
                    .as_ref()
                    .is_some_and(|t| t.value == "Thumbnail failed for crate.glb")
        })
        .map(|n| n.element.id.trim_end_matches("-text").to_owned())
        .unwrap();
    // The row links to its asset and expands into the catalog explanation.
    assert!(shell.ui.find(&format!("{row}-source")).is_some());
    let seq = row.trim_start_matches("log-").to_owned();
    shell
        .run_command(
            &mut editor,
            "console.select",
            &ara_editor::shell::commands::Target::Text(seq.clone()),
        )
        .unwrap();
    assert!(shell.ui.find(&format!("{row}-explain")).is_some());
    // Warnings toggle off hides it; Clear empties the view.
    shell
        .run_command(&mut editor, "console.warnings", &Default::default())
        .unwrap();
    assert!(shell.ui.find(&row).is_none());
    shell
        .run_command(&mut editor, "console.warnings", &Default::default())
        .unwrap();
    shell
        .run_command(&mut editor, "console.clear", &Default::default())
        .unwrap();
    assert!(shell.ui.find("console-empty").is_some());
    // Problems groups the same warning and explains it on demand.
    shell
        .run_command(&mut editor, "window.problems", &Default::default())
        .unwrap();
    let problem = format!("problem-{seq}");
    assert!(shell.ui.find(&problem).is_some());
    shell
        .run_command(
            &mut editor,
            "problems.explain",
            &ara_editor::shell::commands::Target::Text(seq),
        )
        .unwrap();
    assert!(shell.ui.find(&format!("{problem}-explain")).is_some());
}
#[test]
fn prefab_from_selection_overrides_apply_revert_and_open() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell
        .run_command(&mut editor, "create.prefab", &Default::default())
        .unwrap();
    // The subtree became prefabs/salvage_crate.prefab; the scene keeps a linked instance.
    let workspace = editor.host.store().workspace();
    let prefab = workspace
        .find("prefabs/salvage_crate.prefab")
        .unwrap()
        .to_string();
    assert_eq!(
        workspace.doc(&prefab).unwrap().records().len(),
        2,
        "root and child"
    );
    let instance = editor.state.selection[0].clone();
    let scene = workspace.doc("sc_test0001").unwrap();
    assert!(scene.record("e_crate001").is_none());
    assert!(ara_doc::prefab_of(scene.record(&instance).unwrap()).is_some());
    let view = editor.view().unwrap();
    assert_eq!(view.entities().len(), 2, "instance root and its child");
    // Override a field on the instance, then apply it to the prefab.
    editor
        .apply(
            vec![json!({"cmd":"property.set","args":{"entity":instance,"path":"Transform.scale","value":[2,2,2]}})],
            "Scale crate",
            None,
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let overrides = shell.ui.find("prefab-overrides").unwrap();
    assert_eq!(
        shell.ui.nodes[overrides].text.as_ref().unwrap().value,
        "1 override"
    );
    shell
        .run_command(&mut editor, "prefab.apply_all", &Default::default())
        .unwrap();
    let store = editor.host.store();
    let root = store
        .workspace()
        .doc(&prefab)
        .unwrap()
        .records()
        .iter()
        .find(|(_, r)| r.get("parent").is_none())
        .map(|(_, r)| r.clone())
        .unwrap();
    assert_eq!(root["Transform"]["scale"], json!([2, 2, 2]));
    assert!(
        ara_editor::shell::prefab_ops::instance(store, "sc_test0001", &instance)
            .unwrap()
            .overrides
            .is_empty()
    );
    // A new override can be reverted; Open edits the prefab in the Scene view.
    editor
        .apply(
            vec![json!({"cmd":"property.set","args":{"entity":instance,"path":"name","value":"Special crate"}})],
            "Rename crate",
            None,
        )
        .unwrap();
    shell
        .run_command(&mut editor, "prefab.revert_all", &Default::default())
        .unwrap();
    assert!(
        ara_editor::shell::prefab_ops::instance(editor.host.store(), "sc_test0001", &instance)
            .unwrap()
            .overrides
            .is_empty()
    );
    shell
        .run_command(&mut editor, "prefab.open", &Default::default())
        .unwrap();
    assert_eq!(editor.state.doc, prefab);
}
#[test]
fn assets_show_folders_then_files_hide_noise_and_search_everywhere() {
    let project = Project::new();
    std::fs::create_dir_all(project.0.join("models")).unwrap();
    std::fs::create_dir_all(project.0.join("textures/rock")).unwrap();
    std::fs::write(project.0.join("models/crate.gltf"), "{}").unwrap();
    std::fs::write(project.0.join("models/crate.bin"), [0u8; 16]).unwrap();
    std::fs::write(project.0.join("textures/rock/albedo.png"), [0u8; 16]).unwrap();
    std::fs::write(project.0.join("NOTES.md"), "notes").unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The browser scans off the UI thread.
    let start = std::time::Instant::now();
    while !shell.poll_assets_for_test(&mut editor) {
        assert!(start.elapsed().as_secs() < 10, "asset scan never arrived");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let names = |shell: &Shell| -> Vec<String> {
        shell
            .ui
            .nodes
            .iter()
            .filter(|n| n.element.id.starts_with("asset-card-") && n.element.id.ends_with("-name"))
            .filter_map(|n| n.text.as_ref().map(|t| t.value.clone()))
            .collect()
    };
    // Root: the folders, then root files; notes and glTF buffers stay hidden.
    let root = names(&shell);
    assert!(
        root.contains(&"models".to_owned()) && root.contains(&"textures".to_owned()),
        "{root:?}"
    );
    assert!(
        !root
            .iter()
            .any(|n| n.ends_with(".md") || n.ends_with(".bin")),
        "{root:?}"
    );
    editor
        .action("editor.workspace", json!({"folder":"models"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(names(&shell), ["crate.gltf"]);
    shell
        .run_command(&mut editor, "assets.show_all", &Default::default())
        .unwrap();
    assert!(names(&shell).contains(&"crate.bin".to_owned()));
    // A type filter searches every folder.
    editor
        .action(
            "editor.search",
            json!({"panel":"assets","query":"type:texture"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(names(&shell), ["albedo.png"]);
}
#[test]
fn game_view_waits_for_play_and_play_starts_embedded_in_it() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"game"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // Before Play: an empty state with Play; input capture explains what to do first.
    assert!(shell.ui.find("game-empty").is_some());
    assert!(shell.ui.find("game-play").is_some());
    assert!(
        shell
            .run_command(&mut editor, "game.capture", &Default::default())
            .is_err()
    );
    // Aspect presets are registry commands with check marks.
    shell
        .run_command(&mut editor, "game.aspect.16x9", &Default::default())
        .unwrap();
    let label = shell.ui.find("game-aspect-label").unwrap();
    assert_eq!(shell.ui.nodes[label].text.as_ref().unwrap().value, "16:9");
    // Play in the Game view (the default) starts an embedded player at the view's size.
    let mut params = json!({"window": true});
    ara_editor::testing_embed_play(&editor, &mut params);
    assert_eq!(params["window"], false);
    assert!(params["embed"]["width"].as_u64().unwrap() >= 64);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.inGameView","value":false}),
        )
        .unwrap();
    let mut params = json!({"window": true});
    ara_editor::testing_embed_play(&editor, &mut params);
    assert_eq!(params["window"], true);
}
