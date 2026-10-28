#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! The Problems chip counts the project's problems, never this machine's Vulkan loader
//! and overlay-layer warnings (which Problems and Console still list), so it reads the
//! same on every machine and goldens do not depend on it. Its own test binary: the
//! diagnostics go through the process-wide bus.
use ara_editor::{Editor, shell::Shell};
use serde_json::json;
use std::path::Path;

#[test]
fn the_problems_chip_counts_project_problems_not_this_machines_gpu_layers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-editor/problems")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    let write = |path: &str, value: serde_json::Value| {
        std::fs::write(
            if path == "ara.project.json" {
                root.join(path)
            } else {
                root.join(ara_doc::AUTHORING_ROOT).join(path)
            },
            ara_json::to_canonical_string(&value),
        )
        .unwrap();
    };
    write(
        "ara.project.json",
        json!({"name":"Problems","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
    );
    write(
        "levels/main.scene",
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{}}),
    );
    let mut editor = Editor::open(&root).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    ara_diag::Diagnostic::warning(
        ara_diag::codes::GPU_ENVIRONMENT,
        "Layer VK_LAYER_TEST_OVERLAY uses API version 1.2",
    )
    .emit();
    editor.tick();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("chip-problems").is_none());
    let logs = editor
        .action("logs", json!({"severity": "warning", "limit": 10}))
        .unwrap();
    assert_eq!(logs["total"], 1, "Problems and Console still list it");
    // A project problem counts.
    ara_diag::Diagnostic::warning(ara_diag::codes::DEPRECATED, "a project warning").emit();
    editor.tick();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("chip-problems").is_some());
    drop(editor);
    let _ = std::fs::remove_dir_all(&root);
}
