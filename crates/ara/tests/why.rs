//! `ara why` and `ara logs` from the command line, offline (in-process host).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-why-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Runs `ara <args>` in `cwd` as a human; returns (exit code, stdout).
fn ara(cwd: &Path, args: &[&str]) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ara"));
    for key in ["CLAUDECODE", "CODEX_THREAD_ID", "ARA_AUTHOR"] {
        cmd.env_remove(key);
    }
    let out = cmd.current_dir(cwd).args(args).output().expect("ara runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn visibility_uses_the_registered_geometry_schema_for_shapes_and_models() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview4-tests")
        .join(format!("geometry-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch");
    assert_eq!(ara(&dir, &["init"]).0, 0);
    for shape in ["box", "sphere", "capsule", "plane"] {
        let args = serde_json::json!({"name":shape,"components":{"MeshRenderer":{"shape":shape}}})
            .to_string();
        assert_eq!(
            ara(
                &dir,
                &[
                    "do",
                    "entity.create",
                    "--args",
                    &args,
                    "--why",
                    "geometry fixture"
                ]
            )
            .0,
            0
        );
        let (code, text) = ara(
            &dir,
            &[
                "why",
                &format!("name:{shape} not visible"),
                "--full",
                "--json",
            ],
        );
        assert_eq!(code, 0, "{text}");
        let report: Value = serde_json::from_str(&text).expect("report");
        assert!(
            !report["causes"]
                .as_array()
                .expect("causes")
                .iter()
                .any(|c| c["code"] == "ARA3305"),
            "{text}"
        );
        assert!(
            report["checks"]
                .as_array()
                .expect("checks")
                .iter()
                .any(|c| c["name"] == "mesh" && c["status"] == "ok"),
            "{text}"
        );
    }
}

#[test]
fn why_explains_and_its_fix_runs_through_ara_do() {
    let dir = scratch("visible");
    assert_eq!(ara(&dir, &["init", "--name", "why"]).0, 0);
    let root = dir.as_path();
    let (code, _) = ara(
        root,
        &[
            "do",
            "entity.create",
            "--args",
            r#"{"name": "Crate", "components": {"Transform": {"scale": [2, 0, 2]}}}"#,
            "--why",
            "A flattened crate",
        ],
    );
    assert_eq!(code, 0);

    let (code, text) = ara(root, &["why", "name:Crate", "not", "visible"]);
    assert_eq!(code, 0);
    assert!(text.contains("`Transform.scale` is 0 on y"), "{text}");

    let (_, json) = ara(root, &["--json", "why", "name:Crate not visible", "--full"]);
    let report: Value = serde_json::from_str(&json).expect("json report");
    let scale = report["causes"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["code"] == "ARA3303"))
        .expect("the zero-scale cause");
    let fix = &scale["fixes"][0];
    assert_eq!(fix["command"], "property.set");
    let args = serde_json::to_string(&fix["params"]).expect("args");
    let (code, _) = ara(
        root,
        &[
            "do",
            "property.set",
            "--args",
            &args,
            "--why",
            "Apply the ara why fix",
        ],
    );
    assert_eq!(code, 0);
    let (_, text) = ara(root, &["why", "name:Crate not visible"]);
    assert!(!text.contains("ARA3303"), "fixed:\n{text}");

    let (_, text) = ara(root, &["why", "ARA1302"]);
    assert!(text.starts_with("ARA1302 "), "{text}");
    let (_, text) = ara(root, &["why", "--list"]);
    assert!(
        text.contains("visibility") && text.contains("frame"),
        "{text}"
    );
    // Session approvals are debug events, not log lines (the journal keeps them):
    // `--all` includes info, and still lists none.
    let (code, text) = ara(root, &["logs", "--all", "--code", "ARA1812"]);
    assert_eq!(code, 0);
    assert!(!text.contains("ARA1812"), "{text}");
}
