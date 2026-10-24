#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! The Problems chip counts this machine's Vulkan loader and overlay-layer warnings
//! only outside bounded runs, so goldens are the same on every machine. Its own test
//! binary: the diagnostic goes through the process-wide bus.
use ara_editor::{Editor, shell::Shell};
use serde_json::json;
use std::path::Path;

#[test]
fn bounded_runs_leave_host_environment_problems_out_of_the_chip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-editor/problems")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("levels")).unwrap();
    let write = |path: &str, value: serde_json::Value| {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    };
    write(
        "ara.project.json",
        json!({"name":"Problems","main":"levels/main.scene"}),
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
    assert!(
        shell.ui.find("chip-problems").is_some(),
        "an interactive editor shows the host's problems"
    );
    shell.host_problems = false;
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("chip-problems").is_none());
    drop(editor);
    let _ = std::fs::remove_dir_all(&root);
}
