//! Caller identities survive helper reuse and order independent process publications.
mod support;
use ara_script::{ModuleProfile, project};
use serde_json::Value;
use std::path::Path;

fn completed(inventory: &project::Inventory) -> &Value {
    let last = inventory
        .diagnostics
        .iter()
        .filter_map(|d| d.data.as_deref())
        .rfind(|data| data["module"] == "game")
        .expect("module batch");
    assert_eq!(
        last["compile"]["phase"], "completed",
        "publication must preserve the terminal batch boundary"
    );
    inventory
        .diagnostics
        .iter()
        .filter_map(|d| d.data.as_deref())
        .find(|data| data["module"] == "game" && data["compile"]["phase"] == "completed")
        .expect("authoritative module completion")
}

#[test]
fn project_results_preserve_order_through_cached_publication_and_process_replacement() {
    if let Some(root) = std::env::var_os("ARA_LIFECYCLE_CHILD") {
        let inventory =
            project::publish(Path::new(&root), ModuleProfile::Gameplay).expect("child publication");
        println!(
            "LIFECYCLE:{}",
            serde_json::to_string(&completed(&inventory)["compile"]).expect("context JSON")
        );
        return;
    }
    support::build();
    let root = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            support::dotnet()
                .parent()
                .expect("repo")
                .join("target/script-project")
        })
        .join(format!("lifecycle-project-{}", std::process::id()));
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(&authored).expect("source directory");
    let source = authored.join("Counter.cs");
    std::fs::write(
        &source,
        "public static class Counter { public static int Value => Missing; }",
    )
    .expect("broken source");
    let failed = project::publish(&root, ModuleProfile::Gameplay).expect("failed result");
    assert_eq!(completed(&failed)["compile"]["result"], "errors");
    let old = completed(&failed)["compile"].clone();
    std::fs::write(
        &source,
        "public static class Counter { public static int Value => 7; }",
    )
    .expect("repair");
    let repaired = project::publish(&root, ModuleProfile::Gameplay).expect("repair result");
    assert_eq!(completed(&repaired)["compile"]["result"], "success");
    assert_eq!(completed(&repaired)["compile"]["instance"], old["instance"]);
    assert!(completed(&repaired)["compile"]["attempt"].as_u64() > old["attempt"].as_u64());
    let cached = project::publish(&root, ModuleProfile::Gameplay).expect("cached publication");
    assert_eq!(cached.modules[0].result, "cached");
    assert_eq!(
        completed(&cached)["compile"]["validation"],
        "current-module-inputs"
    );
    let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "project_results_preserve_order_through_cached_publication_and_process_replacement",
            "--nocapture",
        ])
        .env("ARA_LIFECYCLE_CHILD", &root)
        .output()
        .expect("second caller process");
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let output = String::from_utf8(child.stdout).expect("test stdout");
    let context: Value = serde_json::from_str(
        output
            .lines()
            .find_map(|line| line.split_once("LIFECYCLE:").map(|(_, value)| value))
            .expect("child context"),
    )
    .expect("JSON context");
    assert_ne!(context["instance"], old["instance"]);
    assert!(context["binding"].as_u64() > old["binding"].as_u64());
    let reclaimed =
        project::publish(&root, ModuleProfile::Gameplay).expect("parent rebinds after replacement");
    assert!(completed(&reclaimed)["compile"]["binding"].as_u64() > context["binding"].as_u64());
}
