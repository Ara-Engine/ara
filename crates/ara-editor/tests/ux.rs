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
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir_all(root.join("settings")).unwrap();
        std::fs::write(
            root.join("settings/ara.imports"),
            ara_asset::ProjectImportSettings::default()
                .canonical_text()
                .unwrap(),
        )
        .unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"UX test","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
        );
        write(
            &root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
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

#[test]
fn copy_reference_uses_the_authored_path_under_the_content_root() {
    use ara_editor::shell::commands::Target;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let before = editor.host.store().revision();
    shell
        .run_command(
            &mut editor,
            "edit.copy_reference",
            &Target::Entity("e_crate001".into()),
        )
        .unwrap();
    let copied = shell
        .take_clipboard()
        .expect("Copy Reference writes the shell clipboard lane");
    assert_eq!(copied, "@levels/main.scene#e_crate001");
    let parts = ara_schema::grammar::parse_ref(&copied).unwrap();
    let workspace = editor.host.store().workspace();
    let physical = workspace.storage_path("levels/main.scene");
    assert_eq!(
        workspace.path_of(&editor.state.doc),
        Some(physical.as_str())
    );
    let scene = workspace.find(parts.path.unwrap()).unwrap();
    assert!(
        workspace
            .doc(scene.as_str())
            .unwrap()
            .record(parts.id.unwrap())
            .is_some(),
        "the copied reference resolves to the actual selected entity"
    );
    assert_eq!(
        editor.host.store().revision(),
        before,
        "Copy Reference is read-only"
    );
}

#[test]
fn import_panel_overlays_live_dependencies_and_applies_settings_with_one_content_prefix() {
    use ara_ui::input::InputEvent;
    use ara_ui_text::EditEvent;
    let project = Project::new();
    let authored = project.0.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(authored.join("models")).unwrap();
    std::fs::write(
        authored.join("models/crate.gltf"),
        r#"{"asset":{"version":"2.0"}}"#,
    )
    .unwrap();
    write(
        &authored.join("models/crate.gltf.import.json"),
        json!({"$schema":"ara://schema/import/1","id":"as_crate001","records":{"i_settings":{"importer":"ara.gltf","version":"1","settings":{}}}}),
    );
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 2100.]).unwrap();
    let started = std::time::Instant::now();
    while !shell.poll_assets_for_test(&mut editor) {
        assert!(started.elapsed().as_secs() < 10, "asset scan never arrived");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The scene's browser snapshot predates this live authored edge.
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"MeshRenderer.model","value":"@models/crate.gltf#as_crate001"}})], "Use the crate model", None).unwrap();
    editor
        .action(
            "editor.workspace",
            json!({"asset":format!("{}/levels/main.scene",ara_doc::AUTHORING_ROOT)}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let label = |shell: &Shell, id: &str| {
        shell.ui.nodes[shell.ui.find(id).unwrap()]
            .element
            .attrs
            .get("label")
            .cloned()
            .unwrap()
    };
    assert_eq!(label(&shell, "dependency-Uses-0"), "models/crate.gltf");
    let mut clipboard = MemoryClipboard::default();
    shell.click("dependency-Uses-0", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        editor.state.asset_path.as_deref(),
        Some(format!("{}/models/crate.gltf", ara_doc::AUTHORING_ROOT).as_str())
    );
    assert!(
        shell.ui.find("import-settings-title").is_some(),
        "the physical sidecar supplies the imported settings"
    );
    assert_eq!(label(&shell, "dependency-Used by-0"), "levels/main.scene");
    let count = &shell.ui.nodes[shell.ui.find("dep-Used by-count").unwrap()];
    assert_eq!(
        count.text.as_ref().unwrap().value,
        "1",
        "the live overlay replaces the same logical graph key without a duplicate physical-path edge"
    );
    shell
        .click("import-setting-lodCount", &mut clipboard)
        .unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)
        .unwrap();
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert("3".into())),
            &mut clipboard,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(shell.commit_fields(&mut editor).unwrap());
    editor.host.flush();
    let sidecar = editor
        .host
        .store()
        .workspace()
        .storage_path("models/crate.gltf.import.json");
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(project.0.join(&sidecar)).unwrap()).unwrap();
    assert_eq!(settings["records"]["i_settings"]["settings"]["lodCount"], 3);
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .index()
            .path_of("as_crate001"),
        Some("models/crate.gltf")
    );
    assert!(
        !authored.join(ara_doc::AUTHORING_ROOT).exists(),
        "Apply must not create content/content"
    );
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
    let camera = editor.state.camera;
    let selection = editor.state.selection.clone();
    let doc = editor.state.doc.clone();
    let content = ara_doc::write(
        editor.host.store().workspace().doc(&doc).unwrap(),
        editor.host.store().schema(),
    );
    let content_revision = editor.host.store().revision();
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
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    assert_eq!(editor.state.doc, doc);
    assert_eq!(editor.host.store().revision(), content_revision);
    assert_eq!(
        ara_doc::write(
            editor.host.store().workspace().doc(&doc).unwrap(),
            editor.host.store().schema()
        ),
        content
    );
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
    // Scene can detach into its own retained surface while a separate Inspector
    // stays open. Redocking either panel must not discard the other's window.
    editor
        .action("editor.select", json!({"entities":selection}))
        .unwrap();
    editor
        .action("editor.layout", json!({"panel":"scene"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.viewport().is_some());
    let camera = editor.state.camera;
    let content_revision = editor.host.store().revision();
    let content_history = editor.host.store().history().len();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"scene","slot":"float","rect":[160,120,900,640]}),
        )
        .unwrap();
    let scene_window = editor
        .state
        .floating
        .iter()
        .find(|window| {
            ara_ui::docking::DockLayout::new(window.root.clone())
                .unwrap()
                .contains("scene")
        })
        .unwrap()
        .id
        .clone();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"inspector","slot":"float","rect":[1080,120,340,640]}),
        )
        .unwrap();
    let inspector_window = editor
        .state
        .floating
        .iter()
        .find(|window| {
            ara_ui::docking::DockLayout::new(window.root.clone())
                .unwrap()
                .contains("inspector")
        })
        .unwrap()
        .clone();
    assert_ne!(scene_window, inspector_window.id);
    assert_eq!(editor.state.floating.len(), 2);
    shell.rebuild(&mut editor).unwrap();
    assert!(
        shell.viewport().is_none(),
        "main must not retain the detached Scene's picking rectangle"
    );
    assert!(shell.enter(&scene_window));
    let viewport = shell
        .viewport()
        .expect("floating Scene owns a real visible viewport");
    assert!(viewport.w > 1. && viewport.h > 1.);
    assert!(shell.ui.find("inspector-scroll").is_none());
    shell.leave();
    assert!(shell.enter(&inspector_window.id));
    assert!(shell.ui.find("inspector-scroll").is_some());
    assert!(shell.viewport().is_none());
    shell.leave();
    shell.drag_tab_to(&mut editor, "scene", "view").unwrap();
    assert_eq!(editor.state.floating, vec![inspector_window.clone()]);
    assert!(
        shell.viewport().is_some(),
        "redocked Scene becomes the main viewport again"
    );
    assert!(
        !shell.enter(&scene_window),
        "the emptied Scene window is removed"
    );
    shell.drag_tab_to(&mut editor, "inspector", "view").unwrap();
    assert!(editor.state.floating.is_empty());
    let dock = ara_ui::docking::DockLayout::new(editor.state.layout.clone()).unwrap();
    assert!(dock.contains("scene") && dock.contains("inspector"));
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    assert_eq!(editor.state.doc, doc);
    assert_eq!(editor.host.store().revision(), content_revision);
    assert_eq!(editor.host.store().history().len(), content_history);
    assert_eq!(
        ara_doc::write(
            editor.host.store().workspace().doc(&doc).unwrap(),
            editor.host.store().schema()
        ),
        content
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
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    assert_eq!(editor.host.store().history().len(), content_history);
    assert_eq!(
        ara_doc::write(
            editor.host.store().workspace().doc(&doc).unwrap(),
            editor.host.store().schema()
        ),
        content
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
    let canonical = ara_host::project_policy::policy(editor.host.store().workspace(), "settings")
        .expect("Project settings enter the authority immediately");
    assert_eq!(canonical["play.tickHz"], "120");
    // The native edit remains nonblocking; disk assertions cross its save receipt.
    editor.host.flush();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(project.0.join("ara.project.json")).unwrap())
            .unwrap();
    let policy = ara_host::project_policy::read(&project.0, "settings")
        .unwrap()
        .unwrap();
    assert_eq!(policy["play.tickHz"], "120");
    assert!(manifest.get("settings").is_none());
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
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
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
        // Tool pages and the Profiler open floating: dock them where the main shell shows them.
        if ara_editor::state::float_size(panel).is_some() {
            editor
                .action("editor.layout.move", json!({"panel":panel,"slot":"view"}))
                .unwrap();
        }
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
fn viewport_toolbar_overlays_the_image_and_first_click_selects_each_tool() {
    use ara_editor::state::Tool;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clipboard = MemoryClipboard::default();
    let viewport = shell.viewport().unwrap();
    for (id, key, tool) in [
        ("tool-Translate", "W", Tool::Translate),
        ("tool-Rotate", "E", Tool::Rotate),
        ("tool-Scale", "R", Tool::Scale),
        ("tool-Universal", "T", Tool::Universal),
    ] {
        let i = shell.ui.find(id).unwrap();
        let node = &shell.ui.nodes[i];
        assert_eq!(node.rect.w, 24.);
        assert_eq!(node.rect.h, 24.);
        assert!(node.rect.y >= viewport.y && node.rect.y + node.rect.h <= viewport.y + 40.);
        assert!(node.element.attrs["description"].contains(key));
        shell.click(id, &mut clipboard).unwrap();
        shell.dispatch(&mut editor).unwrap();
        assert_eq!(editor.state.tool, tool);
        assert!(!editor.state.hide_gizmos);
    }
    shell.click("tool-select", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(editor.state.hide_gizmos);
    shell.click("view-speed", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(
        shell
            .open_menu_rows(&editor)
            .iter()
            .any(|r| r.0 == "view.camera.speed.normal" && r.2 == Some(true))
    );
}
#[test]
fn any_scene_opens_with_its_own_camera_and_agents_only_move_their_presence() {
    let project = Project::new();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/hangar.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_hangar001","records":{"e_lamp0001":{"name":"Lamp","Transform":{"position":[200,2,190]}}}}),
    );
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // File > Open Scene lists every scene, grouped before assets.
    let rows = shell.palette_rows(&mut editor, "#");
    assert!(
        rows.iter().any(|r| r.0 == "Scenes"
            && r.1 == "hangar.scene"
            && r.2.as_deref() == Some("file.open_scene")),
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
    // The Agents panel lists the presence with View and Follow (names with an
    // apostrophe in their tooltip used to break its markup).
    editor
        .action("editor.focus", json!({"panel":"agents"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find(&format!("presence-follow-{agent}")).is_some());
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
    let snapshot = |editor: &Editor| {
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
            .collect::<Vec<_>>()
    };
    let before_create = snapshot(&editor);
    let before_history = editor.host.store().history().len();
    let before_view = editor.view().unwrap();
    let before_poses = ara_editor::world::world_transforms(before_view.entities());
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
    assert_eq!(
        instance, "e_crate001",
        "conversion preserves the selected root identity"
    );
    assert_eq!(scene.records().len(), 1);
    assert!(
        scene.record("e_child001").is_none(),
        "the child now belongs to the prefab source"
    );
    let reference = format!("@prefabs/salvage_crate.prefab#{prefab}");
    assert_eq!(
        ara_doc::prefab_of(scene.record(&instance).unwrap()),
        Some(reference.as_str())
    );
    let source = workspace.doc(&prefab).unwrap();
    let source_root = source.children(None)[0].to_string();
    let source_child = source.children(Some(&source_root))[0].to_string();
    assert_ne!(source_root, instance);
    assert_ne!(source_child, "e_child001");
    let child = format!("{instance}/{source_child}");
    let view = editor.view().unwrap();
    assert_eq!(view.entities().len(), 2, "instance root and its child");
    assert!(view.entities().contains_key(&child));
    let poses = ara_editor::world::world_transforms(view.entities());
    for (old, current) in [
        ("e_crate001", instance.as_str()),
        ("e_child001", child.as_str()),
    ] {
        assert!(
            before_poses[old]
                .to_cols_array()
                .into_iter()
                .zip(poses[current].to_cols_array())
                .all(|(before, after)| (before - after).abs() < 0.00001),
            "conversion preserves world pose of {old}"
        );
    }
    assert_eq!(
        editor.host.store().history().len(),
        before_history + 1,
        "document creation, population and scene replacement are one transaction"
    );
    let create_txn = editor.host.store().history().last().unwrap().id.clone();
    let after_create = snapshot(&editor);
    shell
        .run_command(&mut editor, "edit.undo", &Default::default())
        .unwrap();
    assert_eq!(
        snapshot(&editor),
        before_create,
        "one Undo restores all authored documents"
    );
    assert!(editor.host.store().workspace().doc(&prefab).is_none());
    assert_eq!(
        editor
            .host
            .store()
            .history()
            .last()
            .unwrap()
            .reverts
            .as_ref(),
        Some(&create_txn)
    );
    shell
        .run_command(&mut editor, "edit.redo", &Default::default())
        .unwrap();
    assert_eq!(
        snapshot(&editor),
        after_create,
        "Redo restores the same prefab and instance identities"
    );
    assert_eq!(editor.state.selection, vec![instance.clone()]);
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
    let before_apply = snapshot(&editor);
    let before_apply_history = editor.host.store().history().len();
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
    assert_eq!(
        store.history().len(),
        before_apply_history + 1,
        "source edit and instance cleanup are one transaction"
    );
    let apply_txn = store.history().last().unwrap().id.clone();
    let after_apply = snapshot(&editor);
    shell
        .run_command(&mut editor, "edit.undo", &Default::default())
        .unwrap();
    assert_eq!(
        snapshot(&editor),
        before_apply,
        "one Undo restores both source and instance overrides"
    );
    assert_eq!(
        editor
            .host
            .store()
            .history()
            .last()
            .unwrap()
            .reverts
            .as_ref(),
        Some(&apply_txn)
    );
    assert_eq!(
        ara_editor::shell::prefab_ops::instance(editor.host.store(), "sc_test0001", &instance)
            .unwrap()
            .overrides,
        vec!["Transform.scale"]
    );
    shell
        .run_command(&mut editor, "edit.redo", &Default::default())
        .unwrap();
    assert_eq!(snapshot(&editor), after_apply);
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
    std::fs::create_dir_all(project.0.join(ara_doc::AUTHORING_ROOT).join("models")).unwrap();
    std::fs::create_dir_all(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("textures/rock"),
    )
    .unwrap();
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("models/crate.gltf"),
        "{}",
    )
    .unwrap();
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("models/crate.bin"),
        [0u8; 16],
    )
    .unwrap();
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("textures/rock/albedo.png"),
        [0u8; 16],
    )
    .unwrap();
    std::fs::write(
        project.0.join(ara_doc::AUTHORING_ROOT).join("NOTES.md"),
        "notes",
    )
    .unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action(
            "editor.workspace",
            json!({"folder":ara_doc::AUTHORING_ROOT}),
        )
        .unwrap();
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
    // Tiles show names without extensions; the tooltip starts with the file name.
    let files = |shell: &Shell| -> Vec<String> {
        shell
            .ui
            .nodes
            .iter()
            .filter(|n| {
                n.element
                    .id
                    .strip_prefix("asset-card-")
                    .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
            })
            .filter_map(|n| n.element.attrs.get("description"))
            .filter_map(|d| d.split(" · ").next().map(str::to_owned))
            .collect()
    };
    // Root: the folders, then root files; notes and glTF buffers stay hidden.
    let root = names(&shell);
    assert!(
        root.contains(&"models".to_owned()) && root.contains(&"textures".to_owned()),
        "{root:?}"
    );
    let root_files = files(&shell);
    assert!(
        !root_files
            .iter()
            .any(|n| n.ends_with(".md") || n.ends_with(".bin")),
        "{root_files:?}"
    );
    editor
        .action(
            "editor.workspace",
            json!({"folder":format!("{}/models",ara_doc::AUTHORING_ROOT)}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(names(&shell), ["crate"]);
    assert_eq!(files(&shell), ["crate.gltf"]);
    shell
        .run_command(&mut editor, "assets.show_all", &Default::default())
        .unwrap();
    assert!(files(&shell).contains(&"crate.bin".to_owned()));
    // A type filter searches every folder.
    editor
        .action(
            "editor.search",
            json!({"panel":"assets","query":"type:texture"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(names(&shell), ["albedo"]);

    // The memoized panel follows each of its inputs: under ARA_EDITOR_MEMO_CHECK=1
    // (every `cargo test`) a rebuild that reuses markup an input made stale fails.
    for (action, params) in [
        ("editor.search", json!({"panel":"assets","query":""})),
        (
            "editor.workspace",
            json!({"asset":format!("{}/models/crate.gltf",ara_doc::AUTHORING_ROOT)}),
        ),
        ("editor.workspace", json!({"grid":false})),
        (
            "editor.settings.set",
            json!({"key":"appearance.density","value":"comfortable"}),
        ),
        ("editor.workspace", json!({"grid":true})),
        (
            "editor.settings.set",
            json!({"key":"input.keymap","value":"blender"}),
        ),
    ] {
        editor.action(action, params).unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.rebuild(&mut editor).unwrap();
    }
    shell
        .run_command(&mut editor, "assets.show_all", &Default::default())
        .unwrap();
    assert!(!files(&shell).contains(&"crate.bin".to_owned()));
    // A file that appears on disk shows up once the browser rescans.
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("models/barrel.gltf"),
        "{}",
    )
    .unwrap();
    let start = std::time::Instant::now();
    while !files(&shell).contains(&"barrel.gltf".to_owned()) {
        assert!(
            start.elapsed().as_secs() < 10,
            "the new file never appeared"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        shell.poll_assets_for_test(&mut editor);
    }
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
    // A scene without a Camera says so and offers one; once it has one, the view
    // waits for the camera's first preview frame (`app/game_preview.rs`).
    let mut clip = MemoryClipboard::default();
    shell.click("game-camera", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("game-camera").is_none());
    assert!(shell.ui.find("game-play").is_some());
    // Its preview shows letterboxed to the aspect preset, until Play starts.
    shell
        .run_command(&mut editor, "game.aspect.16x9", &Default::default())
        .unwrap();
    shell.show_game_preview_for_test(&mut editor, [1280, 720]);
    let image = shell.ui.find("game-image").unwrap();
    assert_eq!(
        shell.ui.nodes[image]
            .element
            .attrs
            .get("src")
            .map(String::as_str),
        Some("game-preview")
    );
    let frame = shell.ui.nodes[shell.ui.find("game-frame").unwrap()].rect;
    assert!((frame.w / frame.h - 16. / 9.).abs() < 0.01);
    assert!(shell.ui.find("game-empty").is_none());
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

/// Moving the Scene camera changes no panel: the shell does not rebuild for it, also
/// once the window has detected its GPU preset (a field navigation updates omit).
#[test]
fn camera_moves_do_not_rebuild_the_panels() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor.state.detected_quality = Some(ara_render_fidelity::presets::QualityPreset::High);
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    editor
        .action("editor.camera", json!({"yaw": 0.7, "pitch": 0.3}))
        .unwrap();
    editor.tick();
    assert!(!shell.rebuild_if_stale(&mut editor).unwrap());
    // A panel change still rebuilds.
    editor
        .action("editor.view", json!({"play": "playing"}))
        .unwrap();
    editor.tick();
    assert!(shell.rebuild_if_stale(&mut editor).unwrap());
}

/// Play's embed and the camera preview follow the Game view into a floating window:
/// they size from the window that hosts it.
#[test]
fn a_floating_game_tab_hosts_play_and_the_preview() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel": "game"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let (host, _) = shell.game_host().expect("the Game view in the main window");
    assert_eq!(host, None);
    editor
        .action(
            "editor.layout.move",
            json!({"panel": "game", "slot": "float", "rect": [200, 120, 800, 520]}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let window = editor.state.floating[0].id.clone();
    let (host, frame) = shell.game_host().expect("the Game view in its window");
    assert_eq!(host, Some(window.as_str()));
    assert!(frame[0] > 400. && frame[0] <= 800. && frame[1] > 200. && frame[1] <= 520.);
}

/// Play's accent line draws over the menubar's edge: starting Play never moves the UI.
#[test]
fn starting_play_marks_the_title_bar_without_moving_the_ui() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let dock = |shell: &Shell| {
        let r = shell.ui.nodes[shell.ui.find("dock-root").unwrap()].rect;
        [r.x, r.y, r.w, r.h]
    };
    let before = dock(&shell);
    assert!(shell.ui.find("play-line").is_none());
    editor
        .action("editor.view", json!({"play": "playing"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("play-line").is_some());
    assert_eq!(dock(&shell), before);
}

/// The Scene view's Lit menu offers the project's default, Auto and the four presets
/// with a check on the active one; menus, the palette and agents share the registry
/// commands. A narrow toolbar stays compact; its menu and status show the preset.
#[test]
fn scene_quality_menu_offers_project_auto_and_presets_from_the_registry() {
    use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let text = |shell: &Shell, id: &str| {
        shell
            .ui
            .find(id)
            .and_then(|i| shell.ui.nodes[i].text.as_ref())
            .map(|t| t.value.clone())
            .unwrap_or_default()
    };
    let mut clipboard = MemoryClipboard::default();
    shell.click("view-mode", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let rows: Vec<_> = shell
        .open_menu_rows(&editor)
        .into_iter()
        .filter(|r| r.0.starts_with("view.quality."))
        .collect();
    let ids: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        ids,
        [
            "view.quality.project",
            "view.quality.auto",
            "view.quality.low",
            "view.quality.medium",
            "view.quality.high",
            "view.quality.epic"
        ]
    );
    // An undetected Auto project default; only the active row is checked.
    assert_eq!(rows[0].1, "Project Default (Auto)");
    let checked: Vec<_> = rows.iter().map(|r| r.2).collect();
    assert_eq!(
        checked,
        [
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            Some(false)
        ]
    );
    assert_eq!(editor.state.render_quality, None);

    // The row's command sets the override; `quality` is derived from it.
    shell
        .run_command(&mut editor, "view.quality.epic", &Default::default())
        .unwrap();
    assert_eq!(
        editor.state.render_quality,
        Some(QualityChoice::Preset(QualityPreset::Epic))
    );
    assert!(editor.state.quality);
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(text(&shell, "status-quality-text"), "Epic");
    assert_eq!(text(&shell, "view-mode-label"), "Lit");
    shell.click("view-mode", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(
        shell
            .open_menu_rows(&editor)
            .iter()
            .any(|row| { row.0 == "view.quality.epic" && row.2 == Some(true) })
    );
    shell
        .run_command(&mut editor, "view.quality.low", &Default::default())
        .unwrap();
    assert!(!editor.state.quality, "Low is the raster view");
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(text(&shell, "view-mode-label"), "Lit");
    assert_eq!(text(&shell, "status-quality-text"), "Low");

    // The palette and agents (editor.commands) see the same commands and state.
    assert!(
        shell
            .palette_rows(&mut editor, "Epic")
            .iter()
            .any(|r| r.2.as_deref() == Some("view.quality.epic"))
    );
    let exported = editor.action("editor.commands", json!({})).unwrap();
    let low = exported["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "view.quality.low")
        .unwrap();
    assert_eq!(low["checked"], true);

    // Agents can also set it as a preference.
    editor
        .action("editor.preferences", json!({"renderQuality": "project"}))
        .unwrap();
    assert_eq!(editor.state.render_quality, None);
    assert!(
        editor
            .action("editor.preferences", json!({"renderQuality": "ultra"}))
            .is_err()
    );
}

/// The Camera menu offers the field of view (presets, the current one checked) and
/// Align View to Selected Camera; the toolbar field takes a typed value.
#[test]
fn camera_menu_and_toolbar_field_set_the_scene_field_of_view() {
    use ara_editor::shell::commands::Target;
    use ara_ui::input::InputEvent;
    use ara_ui_text::EditEvent;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clipboard = MemoryClipboard::default();
    shell.click("view-camera", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let rows = shell.open_menu_rows(&editor);
    let ids: Vec<_> = rows.iter().map(|r| r.0.as_str()).take(4).collect();
    assert_eq!(
        ids,
        [
            "view.camera.perspective",
            "view.camera.fov",
            "view.camera.default",
            "view.camera.align"
        ]
    );
    assert_eq!(rows[1].1, "Field of View (50°)…");

    shell
        .run_command(&mut editor, "view.camera.fov", &Target::None)
        .unwrap();
    let presets: Vec<_> = shell
        .open_menu_rows(&editor)
        .into_iter()
        .filter(|r| r.0 == "view.camera.fov.set")
        .collect();
    assert_eq!(presets.len(), 9);
    let checked: Vec<_> = presets
        .iter()
        .filter(|r| r.2 == Some(true))
        .map(|r| r.1.as_str())
        .collect();
    assert_eq!(checked, ["50°"]);
    shell
        .run_command(
            &mut editor,
            "view.camera.fov.set",
            &Target::Text("46".into()),
        )
        .unwrap();
    assert_eq!(editor.state.camera.fov_y_degrees, 46.);

    // A custom value typed into the toolbar field.
    shell.click("view-fov", &mut clipboard).unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)
        .unwrap();
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert("47.5".into())),
            &mut clipboard,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.commit_fields(&mut editor).unwrap();
    assert_eq!(editor.state.camera.fov_y_degrees, 47.5);
    let field = shell.ui.find("view-fov").unwrap();
    assert_eq!(shell.ui.nodes[field].text.as_ref().unwrap().value, "47.5°");

    // Align needs a Camera entity.
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let blocked = shell
        .run_command(&mut editor, "view.camera.align", &Target::None)
        .unwrap_err();
    assert!(
        blocked.to_string().contains("no Camera component"),
        "{blocked}"
    );
}

/// Build Current Profile: the default Windows profile is made by a command (the
/// canonical export preset, runtime player, no machine paths, script modules found
/// by the export), and a project with a C# module can build.
#[test]
fn build_profile_creation_commits_once_and_keeps_csharp_build_available() {
    use ara_editor::shell::commands::Target;
    let project = Project::new();
    let module = ara_host::project::authored_root(&project.0).join("features/gameplay");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("ara.module.toml"), "name = \"Game\"\n").unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let run = shell
        .run_command(&mut editor, "build.run", &Target::None)
        .unwrap_err();
    assert!(run.to_string().contains("Build the project first"), "{run}");

    let before = editor.host.store().history().len();
    shell
        .run_command(&mut editor, "build.profile.create", &Target::None)
        .unwrap();
    let jobs = editor.action("jobs.list", json!({})).unwrap();
    let job = jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["name"] == "build_profile.create")
        .unwrap()["job"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = project.0.join("settings/builds/windows.export.json");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        editor.tick();
        let status = editor.action("jobs.status", json!({"job":job})).unwrap();
        assert!(
            status["status"] == "running" || status["status"] == "done",
            "profile preparation failed: {status}"
        );
        if status["status"] == "done" && path.is_file() {
            assert_eq!(status["result"]["ok"], true);
            assert_eq!(editor.host.store().history().len(), before + 1);
            assert_eq!(
                status["result"]["txn"],
                editor.host.store().history().last().unwrap().id.0
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "profile did not commit: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let text = std::fs::read_to_string(&path).unwrap();
    let preset = ara_export::ExportPreset::from_unpinned_text(&text).unwrap();
    assert_eq!(preset.platform, ara_export::Platform::Windows);
    let authored = ara_host::project::authored_root(editor.host.root());
    let content = authored.strip_prefix(editor.host.root()).unwrap();
    assert_eq!(
        preset.content,
        if content.as_os_str().is_empty() {
            ".".into()
        } else {
            content.to_string_lossy().replace('\\', "/")
        }
    );
    assert_eq!(
        preset.player_template,
        ara_export::preset::PlayerTemplate::Runtime
    );
    assert!(
        preset.templates.is_empty(),
        "pinned at build time, not here"
    );
    assert!(preset.script_project.is_empty());
    // Reopening the UI consumes the committed index, rather than treating the
    // initial asynchronous acknowledgement as a successful filesystem mutation.
    drop(shell);
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !shell.poll_assets_for_test(&mut editor) {
        assert!(
            std::time::Instant::now() < deadline,
            "profile index did not arrive"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        shell
            .run_command(&mut editor, "build.profile.create", &Target::None)
            .is_err(),
        "an existing profile is never overwritten"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    assert_eq!(editor.host.store().history().len(), before + 1);

    std::fs::create_dir_all(project.0.join(ara_doc::AUTHORING_ROOT).join("scripts/Game")).unwrap();
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("scripts/Game/game.module.json"),
        ara_json::to_canonical_string(&json!({"name":"Game"})),
    )
    .unwrap();
    let commands = editor.action("editor.commands", json!({})).unwrap();
    let current = commands["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "build.current")
        .unwrap();
    assert_eq!(current["enabled"], true, "{current}");
}

#[test]
fn inactive_list_tabs_stay_mounted_hidden_and_come_back_as_they_were() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The first asset scan arrives on its own thread; were it to land while Assets is
    // hidden, Assets would rightly render the new files when it comes back.
    let start = std::time::Instant::now();
    while !shell.poll_assets_for_test(&mut editor) {
        assert!(start.elapsed().as_secs() < 10, "asset scan never arrived");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let focus = |shell: &mut Shell, editor: &mut Editor, panel: &str| {
        editor
            .action("editor.focus", json!({ "panel": panel }))
            .unwrap();
        editor.sync();
        shell.rebuild_if_stale(editor).unwrap();
    };
    focus(&mut shell, &mut editor, "assets");
    focus(&mut shell, &mut editor, "console");
    // The Assets body stays mounted but hidden: not laid out, painted or hit.
    let ui = &shell.ui;
    let assets = ui
        .find("body-assets")
        .expect("the inactive Assets tab stays mounted");
    let console = ui.find("body-console").unwrap();
    assert!(!ui.shown(assets) && ui.shown(console));
    let content = ui.find("assets-content").unwrap();
    assert!(!ui.paint_order.contains(&content));
    // Switching back is a visibility change: the Assets body is not mounted again
    // (its nodes keep their layout identities; other parts may change meanwhile).
    let identities = |ui: &ara_ui::Ui| {
        ["body-assets", "assets-content"].map(|id| ui.nodes[ui.find(id).unwrap()].layout_id)
    };
    let before = identities(ui);
    focus(&mut shell, &mut editor, "assets");
    let ui = &shell.ui;
    assert_eq!(identities(ui), before);
    assert!(ui.shown(ui.find("assets-content").unwrap()));
    assert!(!ui.shown(ui.find("body-console").unwrap()));
    // Panels with GPU surfaces render only while active.
    focus(&mut shell, &mut editor, "scene");
    assert!(shell.ui.find("body-game").is_none());
}

/// Rebuild cost by stage for UI-only changes (a toolbar toggle, a dock tab), the
/// click-frame work `--verify-responsiveness` measures on Outpost. Prints averages;
/// run with `--ignored --nocapture`.
#[test]
#[ignore = "timing measurement; run explicitly"]
fn rebuild_cost_by_stage() {
    use ara_editor::shell::commands::Target;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut measure = |label: &str, act: &mut dyn FnMut(&mut Shell, &mut Editor, usize)| {
        for i in 0..10 {
            act(&mut shell, &mut editor, i);
        }
        shell.take_stats();
        let runs = 60;
        let started = std::time::Instant::now();
        for i in 0..runs {
            act(&mut shell, &mut editor, i);
        }
        let total = started.elapsed().as_secs_f64() * 1000. / runs as f64;
        let s = shell.take_stats();
        let per = |v: f64| v / f64::from(s.rebuilds.max(1));
        println!(
            "{label}: {total:.2} ms/act, {} rebuilds; rebuild {:.2} = markup {:.2} + mount {:.2} (reconcile {:.2}, frame {:.2}, rows {:.2}); restyled {} paint {}",
            s.rebuilds,
            per(s.rebuild_ms),
            per(s.rebuild_ms - s.mount_ms),
            per(s.mount_ms),
            per(s.reconcile_ms),
            per(s.frame_ms),
            per(s.rows_ms),
            s.work.restyled / s.rebuilds.max(1) as usize,
            s.work.paint_nodes / s.rebuilds.max(1) as usize,
        );
    };
    measure("toolbar toggle", &mut |shell, editor, _| {
        shell
            .run_command(editor, "view.space", &Target::None)
            .unwrap();
        shell.rebuild_if_stale(editor).unwrap();
    });
    measure("dock tab", &mut |shell, editor, i| {
        let panel = if i % 2 == 0 { "console" } else { "assets" };
        editor
            .action("editor.focus", json!({ "panel": panel }))
            .unwrap();
        editor.sync();
        shell.rebuild_if_stale(editor).unwrap();
    });
}

#[test]
fn axis_widget_caps_are_lettered_and_readable() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    for (cap, letter) in [("view-x", "X"), ("view-y", "Y"), ("view-z", "Z")] {
        let label = shell.ui.find(&format!("{cap}-label")).unwrap();
        let node = &shell.ui.nodes[label];
        assert_eq!(
            node.element.attrs.get("text").map(String::as_str),
            Some(letter)
        );
        let disc = shell.ui.nodes[shell.ui.find(cap).unwrap()]
            .computed
            .background;
        assert_ne!(
            node.computed.color, disc,
            "{cap}: the letter must read on its disc"
        );
    }
    let negative = shell.ui.find("view-x-negative-label").unwrap();
    assert_eq!(
        shell.ui.nodes[negative]
            .element
            .attrs
            .get("text")
            .map(String::as_str),
        Some(""),
        "negative caps are unlettered rings"
    );
}

#[test]
fn this_computers_gpu_messages_stay_out_of_the_project_counts() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action(
            "diag.report",
            json!({"engine": true, "diagnostics": [
                {"code":"ARA_GPU_ENVIRONMENT","severity":"error","message":"loader_get_json: Failed to open JSON file C:/ProgramData/overlay.json"}
            ]}),
        )
        .unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"console"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let text = |shell: &Shell, id: &str| {
        shell.ui.nodes[shell.ui.find(id).unwrap()]
            .text
            .as_ref()
            .map(|t| t.value.clone())
            .unwrap_or_default()
    };
    let loader = |shell: &Shell| {
        shell.ui.nodes.iter().any(|n| {
            n.text
                .as_ref()
                .is_some_and(|t| t.value.starts_with("loader_get_json"))
        })
    };
    assert_eq!(text(&shell, "console-errors-n"), "0", "not a project error");
    assert_eq!(text(&shell, "console-machine-count"), "1");
    assert!(!loader(&shell), "This computer starts collapsed");
    shell
        .run_command(&mut editor, "console.machine", &Default::default())
        .unwrap();
    assert!(loader(&shell), "and opens on demand");
}

#[test]
fn every_dock_splitter_can_be_grabbed_and_dragged() {
    use ara_ui::input::InputEvent;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut clipboard = MemoryClipboard::default();
    let splitters: Vec<String> = shell
        .ui
        .nodes
        .iter()
        .filter(|n| n.element.id.starts_with("splitter-") && n.visible)
        .map(|n| n.element.id.clone())
        .collect();
    assert!(splitters.len() >= 2, "{splitters:?}");
    for id in splitters {
        let i = shell.ui.find(&id).unwrap();
        let r = shell.ui.nodes[i].rect;
        // A splitter spans its whole split: there is something to grab.
        assert!(r.w >= 4. && r.h >= 4., "{id}: {r:?}");
        let centre = [r.x + r.w / 2., r.y + r.h / 2.];
        assert_eq!(shell.ui.hit_test(centre[0], centre[1]), Some(i), "{id}");
        let across = r.w < r.h;
        let path = id.trim_start_matches("splitter-").to_owned();
        let first = |shell: &Shell| {
            let n = &shell.ui.nodes[shell.ui.find(&format!("dock-first-{path}")).unwrap()];
            if across { n.rect.w } else { n.rect.h }
        };
        let before = first(&shell);
        let to = if across {
            [centre[0] + 40., centre[1]]
        } else {
            [centre[0], centre[1] + 40.]
        };
        for event in [
            InputEvent::PointerMove(centre),
            InputEvent::PointerDown(centre),
            InputEvent::PointerMove(to),
            InputEvent::PointerUp(to),
        ] {
            shell.event(event, &mut clipboard).unwrap();
        }
        shell.dispatch(&mut editor).unwrap();
        shell.rebuild(&mut editor).unwrap();
        let after = first(&shell);
        assert!(
            (after - before - 40.).abs() < 6.,
            "{id}: first pane {before} -> {after}"
        );
    }
}

#[test]
fn a_lone_panel_drags_by_its_header_into_another_area() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    // A single-panel area shows a plain header, not tabs.
    let (header, panel) = shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.visible
                && n.element
                    .attrs
                    .get("class")
                    .is_some_and(|c| c == "panel-header")
        })
        .and_then(|n| {
            let panel = n.element.id.strip_prefix("header-")?.to_owned();
            Some((n.rect, panel))
        })
        .expect("the default layout has a single-panel area");
    let area_of = |editor: &Editor, panel: &str| {
        fn find(node: &ara_ui::docking::DockNode, panel: &str) -> Option<String> {
            match node {
                ara_ui::docking::DockNode::Area { id, tabs, .. } => {
                    tabs.iter().any(|t| t == panel).then(|| id.clone())
                }
                ara_ui::docking::DockNode::Split { first, second, .. } => {
                    find(first, panel).or_else(|| find(second, panel))
                }
            }
        }
        find(&editor.state.layout, panel).unwrap()
    };
    let from = area_of(&editor, &panel);
    let target = shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.visible && n.element.id.starts_with("area-") && n.element.id != format!("area-{from}")
        })
        .map(|n| (n.element.id.trim_start_matches("area-").to_owned(), n.rect))
        .unwrap();
    shell.pointer = [header.x + header.w * 0.4, header.y + header.h * 0.5];
    shell.begin_drag();
    assert_eq!(
        shell.dragged_tab(),
        Some(panel.as_str()),
        "{panel}'s header starts a drag"
    );
    shell.pointer = [target.1.x + target.1.w * 0.5, target.1.y + target.1.h * 0.5];
    shell.drag_over();
    assert!(shell.finish_tab_drag(&mut editor, None).unwrap());
    assert_eq!(
        area_of(&editor, &panel),
        target.0,
        "{panel} joined the other area"
    );
}

fn panels(node: &ara_ui::docking::DockNode) -> Vec<(String, Vec<String>)> {
    match node {
        ara_ui::docking::DockNode::Area { id, tabs, .. } => vec![(id.clone(), tabs.clone())],
        ara_ui::docking::DockNode::Split { first, second, .. } => {
            let mut out = panels(first);
            out.extend(panels(second));
            out
        }
    }
}

#[test]
fn the_default_layout_keeps_full_height_sidebars_and_a_center_bottom_dock() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let areas = panels(&editor.state.layout);
    let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    assert_eq!(
        areas,
        [
            ("outline".into(), s(&["hierarchy"])),
            ("view".into(), s(&["scene", "game"])),
            (
                "bottom".into(),
                s(&["assets", "console", "problems", "agents"])
            ),
            ("right".into(), s(&["inspector"])),
        ]
    );
    assert!(editor.state.collapsed_areas.is_empty());
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let rect = |shell: &Shell, id: &str| shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
    let left = rect(&shell, "area-outline");
    let right = rect(&shell, "area-right");
    let view = rect(&shell, "area-view");
    let bottom = rect(&shell, "area-bottom");
    assert!((left.y - right.y).abs() < 1. && (left.h - right.h).abs() < 1.);
    assert!(left.x + left.w <= bottom.x && bottom.x + bottom.w <= right.x);
    assert!((view.x - bottom.x).abs() < 1. && (view.w - bottom.w).abs() < 1.);
    assert!(view.y + view.h < bottom.y && bottom.h > 100.);
    let mut clip = MemoryClipboard::default();
    shell.click("tab-console", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("body-console").is_some());
    shell.click("fold-bottom", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(editor.state.collapsed_areas.contains("bottom"));
    editor.action("editor.layout.reset", json!({})).unwrap();
    assert_eq!(editor.state.layout, ara_editor::state::default_layout());
    assert!(editor.state.collapsed_areas.is_empty());
}

#[test]
fn the_title_bar_shows_project_and_scene_path() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    // The project by its manifest name, not its folder (the loading window too).
    let name = &shell.ui.nodes[shell.ui.find("crumb-project").unwrap()];
    assert_eq!(
        name.text.as_ref().map(|t| t.value.as_str()),
        Some("UX test")
    );
    let crumb = &shell.ui.nodes[shell.ui.find("crumb-scene").unwrap()];
    assert_eq!(
        crumb.text.as_ref().map(|t| t.value.as_str()),
        Some("levels/main.scene")
    );
    assert_eq!(
        crumb.element.attrs.get("description").map(String::as_str),
        Some("levels/main.scene")
    );
}

/// Parts are parsed one by one, so the markup parser cannot see an ID that two
/// panels both declare. Every panel open in one window at once, each in an area of
/// its own (two graph pages side by side included), declares each ID once.
#[test]
fn element_ids_stay_unique_with_every_panel_open_in_one_window() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    for (domain, path) in [
        ("material", "paint.graph"),
        ("vfx", "spark.graph"),
        ("animation", "motion.animation"),
    ] {
        let doc = editor
            .action(
                "editor.resource.create",
                json!({"domain": domain, "path": path}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        editor
            .action("editor.workspace", json!({"resource": doc}))
            .unwrap();
    }
    // Split areas breadth first, so no area ends up much smaller than the rest.
    let mut areas = std::collections::VecDeque::from(["view".to_owned()]);
    for (n, panel) in [
        "game",
        "hierarchy",
        "inspector",
        "assets",
        "console",
        "problems",
        "agents",
        "material",
        "vfx",
        "animation",
        "profiler",
        "settings",
    ]
    .into_iter()
    .enumerate()
    {
        editor
            .action("editor.layout.show", json!({"panel": panel}))
            .unwrap();
        let area = areas.pop_front().unwrap();
        let own = format!("own-{panel}");
        editor
            .action(
                "editor.layout",
                json!({"panel": panel, "area": area, "newArea": own, "vertical": n % 2 == 1}),
            )
            .unwrap();
        areas.extend([area, own]);
    }
    assert!(editor.state.floating.is_empty());
    let mut shell = Shell::new(&mut editor, [3840., 2160.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let repeated: std::collections::BTreeSet<_> = shell
        .ui
        .nodes
        .iter()
        .map(|n| n.element.id.as_str())
        .filter(|id| !seen.insert(*id))
        .collect();
    assert!(repeated.is_empty(), "IDs declared twice: {repeated:?}");
    for canvas in ["material-canvas", "animation-canvas"] {
        assert!(shell.ui.find(canvas).is_some(), "{canvas} is not on screen");
    }
}

/// Material and Animation side by side: each keeps its own canvas and view; a press
/// on one makes it the page the navigation state, graph commands and keys follow.
#[test]
fn two_graph_pages_keep_their_own_view_and_a_press_picks_the_active_one() {
    use ara_platform::Event;
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    for (domain, path) in [
        ("material", "paint.graph"),
        ("animation", "motion.animation"),
    ] {
        let doc = editor
            .action(
                "editor.resource.create",
                json!({"domain": domain, "path": path}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        editor
            .action("editor.workspace", json!({"resource": doc}))
            .unwrap();
    }
    editor
        .action(
            "editor.layout.move",
            json!({"panel": "material", "slot": "view"}),
        )
        .unwrap();
    editor
        .action(
            "editor.layout",
            json!({"panel": "animation", "area": "view", "newArea": "beside"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [2560., 1440.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    let centre = |shell: &Shell, id: &str| {
        let r = shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
        [r.x + r.w * 0.5, r.y + r.h * 0.5]
    };
    let press = |shell: &mut Shell, editor: &mut Editor, at: [f32; 2]| {
        shell.pointer = at;
        for pressed in [true, false] {
            shell
                .graph_event(editor, &Event::MouseButton { button: 1, pressed })
                .unwrap();
        }
        shell.rebuild(editor).unwrap();
    };
    // Animation was docked last, so it has focus: the wheel zooms its canvas.
    assert_eq!(editor.state.focus, "animation");
    shell.pointer = centre(&shell, "animation-canvas");
    shell
        .graph_event(&mut editor, &Event::Wheel([0., 3.]))
        .unwrap();
    let zoomed = editor.state.canvas;
    shell.rebuild(&mut editor).unwrap();
    // A press on the Material canvas makes it active, with its own view.
    let at = centre(&shell, "material-canvas");
    press(&mut shell, &mut editor, at);
    assert_eq!(editor.state.focus, "material");
    assert_ne!(editor.state.canvas, zoomed);
    // Back on Animation, its zoom is where it was left.
    let at = centre(&shell, "animation-canvas");
    press(&mut shell, &mut editor, at);
    assert_eq!(editor.state.focus, "animation");
    assert_eq!(editor.state.canvas, zoomed);
}

#[test]
fn tool_pages_open_floating_and_stay_where_they_are_docked() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    for panel in ["material", "vfx", "animation", "profiler"] {
        assert!(
            panels(&editor.state.layout)
                .iter()
                .all(|(_, tabs)| !tabs.iter().any(|t| t == panel))
        );
        editor
            .action("editor.layout.show", json!({"panel": panel}))
            .unwrap();
        assert!(
            editor.state.floating.iter().any(|f| panels(&f.root)
                .iter()
                .any(|(_, tabs)| tabs.iter().any(|t| t == panel))),
            "{panel} opens in a floating window"
        );
    }
    // Docked, the Material page stays docked when shown again and after a reopen.
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"material","slot":"view"}),
        )
        .unwrap();
    editor
        .action("editor.layout.show", json!({"panel": "material"}))
        .unwrap();
    let docked = |editor: &Editor| {
        panels(&editor.state.layout)
            .iter()
            .any(|(id, tabs)| id == "view" && tabs.iter().any(|t| t == "material"))
    };
    assert!(docked(&editor));
    drop(editor);
    let editor = Editor::open(&project.0).unwrap();
    assert!(docked(&editor));
}

#[test]
fn old_layouts_reset_once_and_current_layouts_preserve_customization() {
    let project = Project::new();
    std::fs::create_dir_all(project.0.join(".ara")).unwrap();
    let path = project.0.join(".ara/editor-layout.json");
    let custom = ara_editor::state::preset_layout("laptop").unwrap();
    // Release layouts were unversioned, including user arrangements and floating windows.
    let legacy = ara_ui::docking::DockLayout::new(custom)
        .unwrap()
        .serialize()
        .unwrap();
    std::fs::write(&path, &legacy).unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    assert_eq!(editor.state.layout, ara_editor::state::default_layout());
    let logs = editor
        .action("logs", json!({"group":false,"limit":100}))
        .unwrap()
        .to_string();
    assert!(logs.contains("Saved layout reset for the new editor layout."));
    drop(editor);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    let mut editor = Editor::open(&project.0).unwrap();
    let logs = editor
        .action("logs", json!({"group":false,"limit":100}))
        .unwrap()
        .to_string();
    assert!(!logs.contains("Saved layout reset for the new editor layout."));
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"console","slot":"float","rect":[50,60,700,400]}),
        )
        .unwrap();
    editor
        .action("editor.layout.save", json!({"name":"custom"}))
        .unwrap();
    let expected = editor.state.clone();
    editor.action("editor.layout.reset", json!({})).unwrap();
    editor
        .action("editor.layout.apply", json!({"name":"custom"}))
        .unwrap();
    assert_eq!(editor.state.layout, expected.layout);
    assert_eq!(editor.state.floating, expected.floating);
    drop(editor);
    let mut editor = Editor::open(&project.0).unwrap();
    assert_eq!(editor.state.layout, expected.layout);
    assert_eq!(editor.state.floating, expected.floating);
    let named = project.0.join(".ara/layouts/legacy.json");
    std::fs::write(&named, legacy).unwrap();
    editor
        .action("editor.layout.apply", json!({"name":"legacy"}))
        .unwrap();
    assert_eq!(editor.state.layout, ara_editor::state::default_layout());
    let upgraded: Value = serde_json::from_str(&std::fs::read_to_string(named).unwrap()).unwrap();
    assert_eq!(upgraded["version"], 2);
    editor
        .action("editor.layout.apply", json!({"name":"legacy"}))
        .unwrap();
    let notices = editor
        .action(
            "logs",
            json!({"codes":["ARA4507"],"group":false,"limit":100}),
        )
        .unwrap();
    assert_eq!(notices["total"], 1);
}

/// Layout changes are saved off the UI thread; however fast they come, the newest
/// one is on disk once the editor closes.
#[test]
fn the_newest_layout_is_saved_when_the_editor_closes() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    for panel in [
        "game", "scene", "game", "console", "problems", "scene", "game",
    ] {
        editor
            .action("editor.layout.show", json!({ "panel": panel }))
            .unwrap();
    }
    let expected = editor.state.layout.clone();
    drop(editor);
    let reopened = Editor::open(&project.0).unwrap();
    assert_eq!(reopened.state.layout, expected);
}
#[test]
fn titlebar_controls_stay_in_one_row_and_logo_opens_about() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    for size in [[960., 600.], [1440., 900.], [1280., 720.]] {
        let mut shell = Shell::new(&mut editor, size).unwrap();
        let rect = |id: &str| shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
        let bar = rect("menubar");
        assert_eq!(bar.h, 36.);
        let play = rect("play-controls");
        assert!((play.x + play.w * 0.5 - size[0] * 0.5).abs() < 1.);
        let menus: Vec<_> = shell
            .ui
            .nodes
            .iter()
            .filter(|n| n.element.id.starts_with("menu-title-") && n.element.tag == "button")
            .collect();
        assert_eq!(menus.len(), 6);
        for node in menus {
            assert!(node.rect.y >= bar.y && node.rect.y + node.rect.h <= bar.y + bar.h);
        }
        let right = rect("caption-buttons");
        assert!((right.x + right.w - size[0]).abs() < 1.);
        assert!(rect("crumbs").x + rect("crumbs").w <= right.x + 1.);
        let bindings = shell.command_bindings();
        for (id, command) in [
            ("brand", "help.about"),
            ("play", "play.start"),
            ("pause", "play.start_paused"),
            ("step", "play.step"),
            ("window-minimize", "window.minimize"),
            ("window-maximize", "window.maximize_window"),
            ("window-close", "file.exit"),
        ] {
            assert!(
                bindings.iter().any(|(i, c)| i == id && *c == command),
                "{id}"
            );
        }
        let mut clip = MemoryClipboard::default();
        shell.click("brand", &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        shell.rebuild(&mut editor).unwrap();
        let version = &shell.ui.nodes[shell.ui.find("dialog-line-0").unwrap()];
        assert_eq!(
            version.text.as_ref().unwrap().value,
            format!("Ara {}", env!("CARGO_PKG_VERSION"))
        );
        shell.click("dialog-close", &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
    }
}
