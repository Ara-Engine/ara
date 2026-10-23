//! Opt-in native GPU golden gate. This opens the production SDL window.
use serde_json::json;
use std::{path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
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

#[test]
#[ignore = "requires a native desktop, Vulkan validation and a compatible GPU"]
fn native_interaction_route_matches_reviewed_dark_and_light_goldens() {
    let project =
        Scratch(std::env::temp_dir().join(format!("ara-editor-golden-{}", std::process::id())));
    std::fs::create_dir(&project.0).unwrap();
    std::fs::create_dir(project.0.join("levels")).unwrap();
    std::fs::write(
        project.0.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"name":"Outpost"})),
    )
    .unwrap();
    std::fs::write(
        project.0.join("levels/main.scene"),
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
    let output = project.0.join(".ara/verification");
    let status = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&project.0)
        .args(["--sample", "--verify", "--frames", "20", "--output"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "native verification failed: {status}");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("interactions.json")).unwrap())
            .unwrap();
    assert_eq!(evidence["verified"], true);
    for theme in ["ara-dark", "ara-light"] {
        let golden = image::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/{theme}.png")),
        )
        .unwrap()
        .into_rgba8();
        let actual = image::open(output.join(format!("{theme}.png")))
            .unwrap()
            .into_rgba8();
        let comparison = ara_render::capture::compare(&golden, &actual, 0.005).unwrap();
        eprintln!("{theme}: 1-SSIM={}", comparison.score);
        if !comparison.passed {
            let evidence =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence/golden-failure");
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
