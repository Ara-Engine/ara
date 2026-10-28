//! Source authoring and folder UX through the production shell and protocol.
#![allow(clippy::unwrap_used)]
use ara_editor::{Editor, shell::Shell};
use ara_ui::input::InputEvent;
use ara_ui_text::EditEvent;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ara-script-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(
            root.join(ara_doc::AUTHORING_ROOT)
                .join("Assets/Gameplay/Empty"),
        )
        .unwrap();
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir_all(root.join("settings")).unwrap();
        std::fs::write(
            root.join("settings/ara.imports"),
            ara_asset::ProjectImportSettings::default()
                .canonical_text()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Script Test","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
            ),
        )
        .unwrap();
        std::fs::write(root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_script01","records":{"e_player01":{"name":"Player","Transform":{}}}}))).unwrap();
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Clip(String);
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, s: &str) -> ara_ui_text::Result<()> {
        self.0 = s.into();
        Ok(())
    }
}
fn loaded(editor: &mut Editor) -> Shell {
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    editor
        .action("editor.workspace", json!({"folder":"Assets/Gameplay"}))
        .unwrap();
    let mut shell = Shell::new(editor, [1440., 900.]).unwrap();
    for _ in 0..100 {
        if shell.poll_assets_for_test(editor) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    shell
}

fn source(root: &std::path::Path, path: &str) -> PathBuf {
    root.join(ara_doc::AUTHORING_ROOT).join(path)
}
fn completed(editor: &mut Editor, response: Value) -> Result<Value, Value> {
    let Some(job) = response["job"].as_str() else {
        return Ok(response);
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        editor.tick();
        let status = editor.action("jobs.status", json!({"job":job})).unwrap();
        match status["status"].as_str() {
            Some("done") => return Ok(status["result"].clone()),
            Some("failed" | "cancelled") => return Err(status),
            _ => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "source job timed out: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
fn wait_sources(editor: &mut Editor) {
    let jobs = editor.action("jobs.list", json!({})).unwrap();
    for job in jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|job| job["name"] == "Author C# source" && job["status"] == "running")
    {
        completed(editor, json!({"job":job["job"]})).unwrap();
    }
}

#[test]
fn typed_script_create_emits_files_in_the_selected_folder_and_refuses_overwrites() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let args = json!({"path":"Assets/Gameplay/PlayerControl.cs","compile":false});
    let output = editor.action("script.create", args.clone()).unwrap();
    let output = completed(&mut editor, output).unwrap();
    assert_eq!(output["class"], "PlayerControl");
    assert_eq!(output["files"].as_array().unwrap().len(), 1);
    let text =
        std::fs::read_to_string(source(&project.0, "Assets/Gameplay/PlayerControl.cs")).unwrap();
    assert!(text.contains("using Ara;"));
    assert!(text.contains(&format!(
        "namespace {};",
        output["namespace"].as_str().unwrap()
    )));
    assert!(text.contains("class PlayerControl : Behaviour"));
    let duplicate = editor.action("script.create", args).unwrap();
    assert!(completed(&mut editor, duplicate).is_err());
    assert_eq!(
        text,
        std::fs::read_to_string(source(&project.0, "Assets/Gameplay/PlayerControl.cs")).unwrap()
    );
    let retry = editor
        .action(
            "script.rename",
            json!({"path":"Assets/Gameplay/PlayerControl.cs","to":"Assets/Gameplay/Recovered.cs"}),
        )
        .unwrap();
    completed(&mut editor, retry).expect("a rejected duplicate must not poison later source jobs");
    assert!(source(&project.0, "Assets/Gameplay/Recovered.cs").is_file());
    let system = editor
        .action(
            "script.create",
            json!({"path":"Assets/Gameplay/Movement.cs","template":"system","compile":false}),
        )
        .unwrap();
    completed(&mut editor, system).unwrap();
    let system =
        std::fs::read_to_string(source(&project.0, "Assets/Gameplay/Movement.cs")).unwrap();
    assert!(system.contains("static partial class Movement"));
    assert!(system.contains("[Query]"));
    for args in [
        json!({"path":"../Escaped.cs"}),
        json!({"path":"Assets/Bad-Name.cs"}),
        json!({"path":".ara/Ignored.cs"}),
        json!({"path":"Assets/Nope.cs","template":"system","attachTo":["e_player01"]}),
        json!({"path":"Assets/Nope.cs","attachTo":["e_missing01"]}),
    ] {
        assert!(editor.action("script.create", args).is_err());
    }
    assert!(
        !project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("Assets/Nope.cs")
            .exists()
    );
}

#[test]
fn folders_start_closed_expand_by_mouse_and_keyboard_and_persist_width() {
    let project = Project::new();
    {
        let mut editor = Editor::open(&project.0).unwrap();
        let mut shell = loaded(&mut editor);
        let mut clip = Clip::default();
        assert!(editor.state.asset_expanded_folders.is_empty());
        let id = format!("asset-folder-{:x}", ara_ui::source::stable_id("Assets"));
        let child = format!(
            "asset-folder-{:x}",
            ara_ui::source::stable_id("Assets/Gameplay")
        );
        assert!(shell.ui.find(&id).is_some());
        assert!(shell.ui.find(&child).is_none());
        shell.click(&format!("{id}-arrow"), &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        assert!(shell.ui.find(&child).is_some());
        shell.click(&child, &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        shell.key(&mut editor, 79, true, &mut clip).unwrap();
        assert!(
            editor
                .state
                .asset_expanded_folders
                .contains("Assets/Gameplay")
        );
        shell.key(&mut editor, 80, true, &mut clip).unwrap();
        assert!(
            !editor
                .state
                .asset_expanded_folders
                .contains("Assets/Gameplay")
        );
        for _ in 0..2 {
            shell.click(&child, &mut clip).unwrap();
            shell.dispatch(&mut editor).unwrap();
        }
        assert!(
            editor
                .state
                .asset_expanded_folders
                .contains("Assets/Gameplay")
        );
        let splitter = shell.ui.nodes[shell.ui.find("asset-sidebar-splitter").unwrap()].rect;
        let content = shell.ui.nodes[shell.ui.find("assets-content").unwrap()].rect;
        let start = [splitter.x + splitter.w * 0.5, splitter.y + splitter.h * 0.5];
        let end = [content.x + 236., start[1]];
        shell
            .event(InputEvent::PointerDown(start), &mut clip)
            .unwrap();
        shell
            .event(InputEvent::PointerMove(end), &mut clip)
            .unwrap();
        shell.event(InputEvent::PointerUp(end), &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        assert!((editor.state.asset_sidebar_width - 236.).abs() < 0.1);
    }
    let editor = Editor::open(&project.0).unwrap();
    assert!((editor.state.asset_sidebar_width - 236.).abs() < 0.1);
    assert!(editor.state.asset_expanded_folders.contains("Assets"));
}

#[test]
fn right_click_create_script_focuses_inline_rename() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = loaded(&mut editor);
    let mut clip = Clip::default();
    assert!(shell.open_assets_context(&mut editor).unwrap());
    shell.click("m0-0", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let script = shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.tag == "button"
                && n.element
                    .attrs
                    .get("label")
                    .is_some_and(|s| s == "C# Script")
        })
        .unwrap()
        .element
        .id
        .clone();
    shell.click(&script, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        shell.input.focused(),
        Some(ara_ui::source::stable_id("asset-rename-value"))
    );
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert("PlayerController".into())),
            &mut clip,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    // Compilation runs in its own executable; this focused test checks the command's file output.
    shell.key(&mut editor, 40, true, &mut clip).unwrap();
    wait_sources(&mut editor);
    let text =
        std::fs::read_to_string(source(&project.0, "Assets/Gameplay/PlayerController.cs")).unwrap();
    assert!(text.contains("class PlayerController : Behaviour"));
    assert!(!source(&project.0, "Assets/Gameplay/NewBehaviour.cs").exists());
    assert!(
        editor
            .host
            .store()
            .schema()
            .get("PlayerController")
            .is_some(),
        "the queued rename compiles its new inventory"
    );
}

#[test]
fn component_new_script_is_registered_for_the_dropdown_and_agents() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_player01"]}))
        .unwrap();
    let mut shell = loaded(&mut editor);
    let commands = editor.action("editor.commands", json!({})).unwrap();
    assert!(
        commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["id"] == "component.new_script" && command["enabled"] == true)
    );
    shell
        .run_command(
            &mut editor,
            "component.new_script",
            &ara_editor::shell::commands::Target::None,
        )
        .unwrap();
    assert_eq!(
        shell.input.focused(),
        Some(ara_ui::source::stable_id("asset-rename-value"))
    );
    let result = editor
        .action(
            "component.new_script",
            json!({"template":"behaviour","attachTo":["e_player01"]}),
        )
        .unwrap();
    assert_eq!(
        result["path"],
        format!(
            "{}/Assets/Gameplay/NewBehaviour2.cs",
            ara_doc::AUTHORING_ROOT
        )
    );
    assert!(result["job"].is_string());
    wait_sources(&mut editor);
    assert!(source(&project.0, "Assets/Gameplay/NewBehaviour.cs").is_file());
    completed(&mut editor, result).unwrap();
    assert!(source(&project.0, "Assets/Gameplay/NewBehaviour2.cs").is_file());
    assert!(
        editor
            .action("component.new_script", json!({"attachTo":[]}))
            .is_err()
    );
}

#[test]
#[ignore = "requires the built editor executable and .NET host; offscreen native route"]
fn native_create_rename_compile_and_attach() {
    let project = Project::new();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let output = target.join("evidence/assets-scripts");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&project.0)
        .args(["--verify-assets", "--output"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("assets-scripts.json")).unwrap())
            .unwrap();
    assert_eq!(report["compiled"], true);
    assert_eq!(report["attached"], true);
    assert_eq!(report["hotCompiled"], true);
}

#[test]
fn asset_grid_scrollbar_drives_a_bounded_card_pool() {
    let project = Project::new();
    for n in 0..500 {
        std::fs::write(
            project
                .0
                .join(ara_doc::AUTHORING_ROOT)
                .join(format!("Assets/Gameplay/Code{n:04}.cs")),
            "// Browser fixture\n",
        )
        .unwrap();
    }
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = loaded(&mut editor);
    let mut clip = Clip::default();
    let grid = shell.ui.find("asset-grid").unwrap();
    let bar = shell
        .ui
        .scrollbars(grid)
        .unwrap()
        .into_iter()
        .find(|b| b.axis == 1)
        .unwrap();
    let start = [
        bar.thumb.x + bar.thumb.w * 0.5,
        bar.thumb.y + bar.thumb.h * 0.5,
    ];
    let end = [start[0], bar.track.y + bar.track.h - 2.];
    shell
        .event(InputEvent::PointerDown(start), &mut clip)
        .unwrap();
    shell
        .event(InputEvent::PointerMove(end), &mut clip)
        .unwrap();
    shell.event(InputEvent::PointerUp(end), &mut clip).unwrap();
    shell.poll_assets_for_test(&mut editor);
    let grid = shell.ui.find("asset-grid").unwrap();
    assert!(shell.ui.scroll_position(grid).unwrap()[1] > 100.);
    assert!(shell.ui.find("asset-card-0").is_none());
    let cards = shell
        .ui
        .nodes
        .iter()
        .filter(|n| n.element.tag == "button" && n.element.id.starts_with("asset-card-"))
        .count();
    assert!(
        cards > 0 && cards < 100,
        "card pool should stay bounded: {cards}"
    );
}
