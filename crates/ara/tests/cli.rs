//! `ara fmt`, `check`, `schema` and `merge` on the fixture projects.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Runs `ara --root <root> <args>`; returns (exit code, stdout).
fn ara(root: &str, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ara"))
        .arg("--root")
        .arg(fixtures().join(root))
        .args(args)
        .output()
        .expect("ara runs");
    let stdout = String::from_utf8(out.stdout).expect("utf-8 output");
    (out.status.code().unwrap_or(-1), stdout)
}

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn path(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

#[test]
fn sample_project_checks_clean_and_is_canonical() {
    let (code, out) = ara("project", &["check"]);
    assert_eq!(
        (code, out.trim()),
        (0, "checked 3 files: 0 errors, 0 warnings")
    );

    let docs = [
        "levels/arena.scene",
        "prefabs/crate.prefab",
        "prefabs/stack.prefab",
    ]
    .map(|d| fixtures().join("project").join(d));
    let args: Vec<&str> = ["fmt", "--check"]
        .into_iter()
        .chain(docs.iter().map(|d| path(d)))
        .collect();
    let (code, out) = ara("project", &args);
    assert_eq!((code, out.trim()), (0, "3 files: would reformat 0"));
}

#[test]
fn fmt_canonicalizes_and_repairs_in_place() {
    let dir = scratch("fmt");
    let file = dir.join("messy.scene");
    fs::copy(fixtures().join("messy.scene"), &file).expect("copy");

    let (code, _) = ara("project", &["fmt", "--check", path(&file)]);
    assert_eq!(code, 1, "unformatted files fail --check");
    let (code, _) = ara("project", &["fmt", path(&file)]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(fs::read_to_string(&file).expect("formatted"));
    let (code, _) = ara("project", &["fmt", "--check", path(&file)]);
    assert_eq!(code, 0, "formatting is idempotent");
}

#[test]
fn check_json_reports_located_diagnostics_with_fixes() {
    let (code, out) = ara("broken", &["check", "--json"]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn schema_dumps_the_reflection_db_and_json_schemas() {
    let (code, out) = ara("project", &["schema", "--json"]);
    assert_eq!(code, 0);
    let reflection = ara_json_parse(&out);
    for key in [
        "/types/Transform",
        "/types/Light",
        "/types/EntityCreateParams",
        "/documents/scene",
        "/commands/entity.create",
    ] {
        assert!(reflection.pointer(key).is_some(), "missing {key}");
    }
    insta::assert_snapshot!("reflection", out);

    let dir = scratch("schema");
    let (code, out) = ara("project", &["schema", "--out", path(&dir)]);
    assert_eq!(code, 0, "{out}");
    for file in [
        "reflection.json",
        "schema/scene.schema.json",
        "schema/prefab.schema.json",
        "schema/types/Transform.schema.json",
    ] {
        let text = fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let v = ara_json_parse(&text);
        assert!(v.is_object(), "{file}");
    }
    let scene = ara_json_parse(
        &fs::read_to_string(dir.join("schema/scene.schema.json")).expect("scene schema"),
    );
    assert_eq!(
        scene["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert!(
        scene.pointer("/$defs/Entity/properties/Light").is_some(),
        "dynamic components are in the document schema"
    );
}

#[test]
fn schema_describes_named_types_and_commands() {
    let (code, out) = ara("project", &["schema", "Light", "entity.create"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("# Component `Light`") && out.contains("| `intensity` | `f32` |"),
        "{out}"
    );
    assert!(out.contains("entity.create"), "{out}");
    let (code, out) = ara("project", &["schema", "Lights"]);
    assert_eq!(code, 1);
    assert!(out.contains("did you mean `Light`?"), "{out}");
}

#[test]
fn merge_writes_the_result_over_ours_and_reports_conflicts() {
    let merge = fixtures().join("merge");
    let dir = scratch("merge");
    let ours = dir.join("ours.scene");
    fs::copy(merge.join("ours.scene"), &ours).expect("copy");
    let (code, out) = ara(
        "project",
        &[
            "merge",
            "--json",
            path(&merge.join("base.scene")),
            path(&ours),
            path(&merge.join("theirs.scene")),
        ],
    );
    assert_eq!(code, 1, "a conflict makes git stop: {out}");
    insta::assert_snapshot!(fs::read_to_string(&ours).expect("merged"));

    // Theirs unchanged from base: the result is ours, cleanly.
    let out_file = dir.join("clean.scene");
    let (code, _) = ara(
        "project",
        &[
            "merge",
            "-o",
            path(&out_file),
            path(&merge.join("base.scene")),
            path(&merge.join("ours.scene")),
            path(&merge.join("base.scene")),
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(&out_file).expect("merged"),
        fs::read_to_string(merge.join("ours.scene")).expect("ours")
    );
}

fn ara_json_parse(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("valid JSON output")
}

/// Runs `ara --root <root> <args>` in a scratch project; returns (exit code, stdout).
fn ara_at(root: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ara"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .output()
        .expect("ara runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn conflicts_resolve_and_fixes_apply_without_hand_editing() {
    let merge = fixtures().join("merge");
    let root = scratch("resolve");
    fs::write(root.join("ara.project.json"), "{ \"name\": \"r\" }\n").expect("marker");
    fs::create_dir_all(root.join("levels")).expect("levels");
    let conf = root.join("levels/conf.scene");
    let input = |n: &str| merge.join(n).to_string_lossy().into_owned();
    let (code, _) = ara_at(
        &root,
        &[
            "merge",
            "-o",
            path(&conf),
            &input("base.scene"),
            &input("ours.scene"),
            &input("theirs.scene"),
        ],
    );
    assert_eq!(code, 1, "one conflict");

    let (code, out) = ara_at(&root, &["check", "--json"]);
    assert_eq!(code, 1);
    let fixes = &ara_json_parse(&out)["diagnostics"][0]["fixes"];
    assert_eq!(
        fixes[1]["args"],
        serde_json::json!(["resolve", "levels/conf.scene", "--pick", "e_b2:name=theirs"])
    );
    let (code, out) = ara_at(
        &root,
        &["resolve", "levels/conf.scene", "--pick", "e_b2:name=theirs"],
    );
    assert_eq!(code, 0, "{out}");
    let text = fs::read_to_string(&conf).expect("resolved");
    assert!(
        !text.contains("$conflicts") && text.contains("Barrel (theirs)"),
        "{text}"
    );

    fs::write(&conf, text.replace("\"order\": \"a1\"", "\"ordr\": \"a1\"")).expect("typo");
    let (_, out) = ara_at(&root, &["check"]);
    assert!(
        out.contains("hint: 1 of these have one machine-applicable fix"),
        "{out}"
    );
    let (code, out) = ara_at(&root, &["check", "--fix"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("fixed levels/conf.scene: Rename `ordr` to `order`"),
        "{out}"
    );
    assert_eq!(fs::read_to_string(&conf).expect("fixed"), text);
}
