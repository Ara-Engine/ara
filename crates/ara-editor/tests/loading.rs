#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! Loading UX from the asset pipeline's import progress: the status bar's Importing
//! chip, Assets badges and the Scene view's loading notice, while imports are pending
//! and once they have failed.
use ara_editor::{Editor, shell::Shell};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A scene whose crate uses `models/crate.gltf`; `models/rock.png` is imported but unused.
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-editor/loading")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("levels")).unwrap();
        std::fs::create_dir_all(root.join("models")).unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"Loading test","main":"levels/main.scene"}),
        );
        write(
            &root.join("levels/main.scene"),
            json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
                "e_crate001":{"name":"Salvage crate","Transform":{},
                    "MeshRenderer":{"model":"@models/crate.gltf#as_crate0001"}}}}),
        );
        for (file, id, importer) in [
            ("crate.gltf", "as_crate0001", "ara.gltf"),
            ("rock.png", "as_rock00001", "ara.image"),
        ] {
            std::fs::write(root.join("models").join(file), "{}").unwrap();
            write(
                &root.join(format!("models/{file}.import.json")),
                json!({"$schema":"ara://schema/import/1","id":id,"records":{"i_settings":{"importer":importer,"version":"1","settings":{}}}}),
            );
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, value: Value) {
    std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
}
/// The text of node `id`, if it is mounted.
fn text(shell: &Shell, id: &str) -> Option<String> {
    let i = shell.ui.find(id)?;
    shell.ui.nodes[i].text.as_ref().map(|t| t.value.clone())
}
/// The Assets card showing `name`.
fn card(shell: &Shell, name: &str) -> String {
    shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.id.starts_with("asset-card-")
                && n.element.id.ends_with("-name")
                && n.text.as_ref().is_some_and(|t| t.value == name)
        })
        .map(|n| n.element.id.trim_end_matches("-name").to_owned())
        .unwrap_or_else(|| panic!("no card for {name}"))
}

#[test]
fn import_progress_shows_in_the_status_bar_assets_and_the_scene() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    editor
        .action("editor.workspace", json!({"folder":"models"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let start = Instant::now();
    while !shell.poll_assets_for_test(&mut editor) {
        assert!(start.elapsed().as_secs() < 10, "asset scan never arrived");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Before the pipeline starts there is nothing to report.
    assert!(shell.ui.find("status-imports").is_none());
    assert!(shell.ui.find("scene-loading").is_none());
    // The pipeline owner, as the play host runs it. Its worker is absent, so both
    // imports are pending from the start and then fail.
    let mut owner =
        ara_play_host::assets::Assets::new(&project.0, &project.0.join("absent-worker.exe"))
            .unwrap();
    assert!(shell.poll_assets_for_test(&mut editor));
    assert_eq!(
        text(&shell, "status-imports-text").as_deref(),
        Some("Importing 0 / 2")
    );
    // Only the crate's model is in the open scene.
    assert_eq!(
        text(&shell, "scene-loading-text").as_deref(),
        Some("Loading 1 asset…")
    );
    let crate_card = card(&shell, "crate.gltf");
    assert_eq!(
        text(&shell, &format!("{crate_card}-kind")).as_deref(),
        Some("Importing…")
    );
    assert!(shell.ui.find(&format!("{crate_card}-import")).is_some());
    let start = Instant::now();
    while text(&shell, "status-imports-text").as_deref() != Some("2 imports failed") {
        assert!(
            start.elapsed().as_secs() < 20,
            "imports never failed: {:?}",
            text(&shell, "status-imports-text")
        );
        owner.poll(false, true);
        shell.poll_assets_for_test(&mut editor);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        text(&shell, "scene-loading-text").as_deref(),
        Some("1 asset failed to import; showing placeholders")
    );
    let crate_card = card(&shell, "crate.gltf");
    assert_eq!(
        text(&shell, &format!("{crate_card}-kind")).as_deref(),
        Some("Import failed")
    );
}
