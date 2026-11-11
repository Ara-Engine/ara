//! Source authoring and folder UX through the production shell and protocol.
#![allow(clippy::unwrap_used)]
use ara_editor::Editor;
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


