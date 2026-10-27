//! The fault seam cannot arm ordinary editor shells or unrelated offscreen captures.
#![allow(clippy::unwrap_used)]
use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn panel_probe_requires_both_offscreen_and_isolated_fixture_identity() {
    let root = crate::test_target().join("ux-boundary-probe").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("levels")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Probe guard","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_probeguard","records":{}}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    }
    let mut editor = Editor::open_with(&root, Some(root.join(".ara/test-user.json"))).unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    std::fs::write(root.join(".ara-behaviour-fixture"), "isolated fixture\n").unwrap();
    shell.offscreen = false;
    assert!(
        shell
            .arm_panel_probe(&editor, "inspector", ProbeFailure::Error, "ordinary")
            .is_err()
    );
    assert!(!shell.panel_probe_armed());
    shell.offscreen = true;
    std::fs::remove_file(root.join(".ara-behaviour-fixture")).unwrap();
    assert!(
        shell
            .arm_panel_probe(
                &editor,
                "inspector",
                ProbeFailure::Panic,
                "unrelated-capture"
            )
            .is_err()
    );
    assert!(!shell.panel_probe_armed());
    std::fs::write(root.join(".ara-behaviour-fixture"), "isolated fixture\n").unwrap();
    shell
        .arm_panel_probe(&editor, "inspector", ProbeFailure::Error, "native-story")
        .unwrap();
    assert!(shell.panel_probe_armed());
    shell.disarm_panel_probe();
    assert!(!shell.panel_probe_armed());
}
