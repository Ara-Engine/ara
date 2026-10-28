//! Opt-in native GPU golden gate: the production UI through `--verify`, a bounded run
//! that renders offscreen. `ARA_EDITOR_UPDATE_GOLDENS=1` rewrites the reviewed goldens.
#![allow(clippy::unwrap_used)] // A failed fixture/capture operation should fail the test immediately.
use serde_json::json;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn target_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("ara-editor-native")
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        assert!(self.0.starts_with(target_root()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ara-editor-golden-")
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh one-scene project under `target/`, removed on drop.
fn fixture() -> Scratch {
    let project = Scratch(target_root().join(format!(
        "ara-editor-golden-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&project.0).unwrap();
    std::fs::create_dir_all(project.0.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    std::fs::write(
        project.0.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"name":"Outpost","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"})),
    )
    .unwrap();
    std::fs::write(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_golden00","records":{}}),
        ),
    )
    .unwrap();
    std::fs::write(
        project.0.join("AGENTS.md"),
        "# Outpost\nNative editor verification fixture.\n",
    )
    .unwrap();
    project
}

fn run_verification() -> (Scratch, PathBuf) {
    let project = fixture();
    let output = project.0.join(".ara/verification");
    let status = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&project.0)
        .args(["--sample", "--verify", "--frames", "256", "--output"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "native verification failed: {status}");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("interactions.json")).unwrap())
            .unwrap();
    assert_eq!(evidence["verified"], true);
    (project, output)
}

#[test]
#[ignore = "requires Vulkan validation and a compatible GPU; every window stays hidden"]
fn native_titlebar_and_layout_interaction_route() {
    let (_project, output) = run_verification();
    let chrome: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("chrome.json")).unwrap())
            .unwrap();
    assert_eq!(chrome["verified"], true);
    assert_eq!(chrome["logoAbout"], true);
    assert_eq!(chrome["fullHeightSidebars"], true);
    assert_eq!(chrome["centerBottomDock"], true);
    #[cfg(windows)]
    assert_eq!(chrome["nativeHitTest"]["maximize"], 9);
}

#[test]
#[ignore = "requires Vulkan validation and a compatible GPU; every window stays hidden"]
fn native_interaction_route_matches_reviewed_dark_and_light_goldens() {
    let (_project, output) = run_verification();
    for theme in ["ara-dark", "ara-light", "palette-dark", "palette-light"] {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/{theme}.png"));
        let actual = image::open(output.join(format!("{theme}.png")))
            .unwrap()
            .into_rgba8();
        if std::env::var_os("ARA_EDITOR_UPDATE_GOLDENS").is_some_and(|v| v == "1") {
            actual.save(&path).unwrap();
        }
        let golden = image::open(path).unwrap().into_rgba8();
        let comparison = ara_render::capture::compare(&golden, &actual, 0.005).unwrap();
        eprintln!("{theme}: 1-SSIM={}", comparison.score);
        if !comparison.passed {
            let evidence = target_root().join("golden-failure");
            std::fs::create_dir_all(&evidence).unwrap();
            actual.save(evidence.join(format!("{theme}.png"))).unwrap();
            comparison
                .diff
                .save(evidence.join(format!("{theme}.diff.png")))
                .unwrap();
        }
        assert!(
            comparison.passed,
            "{theme} differs from the reviewed golden: {}",
            comparison.score
        );
    }
}

/// The focused route drives tabs/titles across hidden native windows at both UI scales.
#[test]
#[ignore = "requires Vulkan validation and a compatible GPU; every window stays hidden"]
fn native_docking_drags_match_drop_previews() {
    let project =
        Scratch(target_root().join(format!("ara-editor-golden-docking-{}", std::process::id())));
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/docking-native")
        .join(std::process::id().to_string());
    let status = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&project.0)
        .args(["--verify-docking", "--frames", "2000", "--output"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "native docking failed: {status}; evidence: {}",
        output.display()
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("drags.json")).unwrap()).unwrap();
    let drags = report["drags"].as_array().unwrap();
    assert!(drags.len() >= 29);
    for drag in drags {
        assert_eq!(drag["ok"], true, "{drag}");
    }
    assert!(output.join("dock-04-preview.png").exists());
}

/// A window that cannot show (the probe holds it minimized or occluded) renders and
/// presents nothing, keeps every loop frame short (a stalled present path never
/// blocks the event pump) and presents again once it can show.
#[test]
#[ignore = "requires Vulkan validation and a compatible GPU; every window stays hidden"]
fn an_unpresentable_window_never_blocks_the_editor_pump() {
    let project = fixture();
    let output = project.0.join(".ara/verification");
    let status = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&project.0)
        .args(["--verify-unpresentable", "--output"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "unpresentable route failed: {status}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("responsiveness.json")).unwrap())
            .unwrap();
    let hidden = &report["unpresentable"];
    assert_eq!(hidden["presented"], 0, "{hidden}");
    assert_eq!(hidden["resumed"], true, "{hidden}");
    assert_eq!(hidden["ok"], true, "{hidden}");
}
