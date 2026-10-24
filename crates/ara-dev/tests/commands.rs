//! Exercise real git/Cargo transactions in disposable repositories under target/.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch(label: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let path = target.join("ara-dev/tests").join(format!(
        "{label}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    let path = path.canonicalize().unwrap();
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\\?\"));
    path
}

fn configured_command(root: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_BUILD_BUILD_DIR", root.join("target"))
        .env("CARGO_BUILD_JOBS", "2")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"));
    command
}

fn command(root: &Path, program: &str, args: &[&str]) -> Output {
    configured_command(root, program, args).output().unwrap()
}

fn ok(root: &Path, program: &str, args: &[&str]) -> Output {
    let out = command(root, program, args);
    assert!(
        out.status.success(),
        "{program} {args:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(root: &Path, args: &[&str]) -> Output {
    ok(root, "git", args)
}

fn fixture() -> PathBuf {
    let root = scratch("repo");
    fs::write(root.join(".gitignore"), "/target/\n/empty-gitconfig\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"core\", \"tool\", \"diag\"]\n",
    )
    .unwrap();
    for (dir, name, extra) in [
        ("core", "mini-core", ""),
        (
            "tool",
            "mini-tool",
            "\n[dependencies]\nrenamed = { package = \"mini-core\", path = \"../core\", optional = true }\n",
        ),
        ("diag", "ara-diag", ""),
    ] {
        fs::create_dir_all(root.join(dir).join("src")).unwrap();
        fs::write(
            root.join(dir).join("Cargo.toml"),
            format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}"),
        )
        .unwrap();
        fs::write(
            root.join(dir).join("src/lib.rs"),
            "pub fn value() -> u32 {\n    1\n}\n",
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("diag/tests")).unwrap();
    fs::write(
        root.join("diag/tests/coverage.rs"),
        "#[test]\nfn catalog() {\n    assert_eq!(ara_diag::value(), 1);\n}\n",
    )
    .unwrap();
    ok(&root, "cargo", &["generate-lockfile", "--offline"]);
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.email", "ara-dev@example.invalid"]);
    git(&root, &["config", "user.name", "ara-dev tests"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fixture"]);
    root
}

#[test]
fn cargo_outputs_stay_local_and_foreign_targets_are_reported_before_writes() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    let other = scratch("foreign-checkout");
    fs::create_dir_all(other.join(".git")).unwrap();
    let foreign = other.join("target/nested-cache");
    let binary = env!("CARGO_BIN_EXE_ara-dev");
    for args in [
        vec!["check", "--base", "HEAD", "--plan", "--json"],
        vec!["merge", "HEAD"],
        vec!["disk", "--warn-gib", "0"],
        vec![
            "gpu",
            "--timeout",
            "1",
            "--",
            binary,
            "disk",
            "--warn-gib",
            "0",
        ],
    ] {
        let output = configured_command(&root, binary, &args)
            .env("LOCALAPPDATA", root.join("target/gpu-path-test"))
            .env("CARGO_BUILD_BUILD_DIR", &foreign)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("another checkout's target"), "{text}");
        assert!(!foreign.exists());
        assert!(!root.join(".git/MERGE_HEAD").exists());
    }
    fs::create_dir_all(root.join(".cargo")).unwrap();
    fs::write(
        root.join(".cargo/config.toml"),
        format!(
            "[build]\ntarget-dir = {0:?}\nbuild-dir = {0:?}\n",
            foreign.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();
    let plan = configured_command(
        &root,
        binary,
        &["check", "--base", "HEAD", "--plan", "--json"],
    )
    .env_remove("CARGO_TARGET_DIR")
    .env_remove("CARGO_BUILD_TARGET_DIR")
    .env_remove("CARGO_BUILD_BUILD_DIR")
    .output()
    .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let run = fs::read_dir(root.join("target/ara-dev"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("metadata.json")).unwrap()).unwrap();
    for name in ["target_directory", "build_directory"] {
        assert_eq!(
            Path::new(metadata[name].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            root.join("target").canonicalize().unwrap()
        );
    }
    let disk = configured_command(&root, binary, &["disk", "--warn-gib", "0"])
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .output()
        .unwrap();
    assert!(!disk.status.success());
    assert!(String::from_utf8_lossy(&disk.stderr).contains("another checkout's target"));
    assert!(!foreign.exists());
}

#[test]
fn check_selects_reverse_dependencies_and_keeps_json_stdout_clean() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    let before = git(&root, &["status", "--porcelain"]).stdout;
    let output = ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["check", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["affected"],
        serde_json::json!(["mini-core", "mini-tool"])
    );
    assert_eq!(before, git(&root, &["status", "--porcelain"]).stdout);
}

#[test]
fn test_helpers_receive_fresh_peer_binaries_with_separate_build_directories() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::write(
        root.join("tool/src/main.rs"),
        "fn main() { println!(\"old peer\"); }\n",
    )
    .unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "peer binary"]);
    ok(&root, "cargo", &["build", "-p", "mini-tool", "--offline"]);
    fs::write(
        root.join("tool/src/main.rs"),
        "fn main() {\n    println!(\"fresh peer\");\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("core/tests")).unwrap();
    fs::write(
        root.join("core/tests/peer.rs"),
        r#"#[test]
fn invokes_fresh_peer() {
    let peer = std::env::var_os("CARGO_BIN_EXE_mini-tool").expect("peer binary");
    let output = std::process::Command::new(peer).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"fresh peer\n");
}
"#,
    )
    .unwrap();
    let output = configured_command(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--json", "--no-compiler-cache"],
    )
    .env("CARGO_BUILD_BUILD_DIR", root.join("target/intermediates"))
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["affected"],
        serde_json::json!(["mini-core", "mini-tool"])
    );
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["check"] == "test-build" && row["passed"] == true)
    );
}

#[test]
fn peer_binaries_share_the_test_feature_union_without_a_second_build() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "{}\n[profile.test]\ndebug-assertions = false\n",
            manifest.replace("\"diag\"]", "\"diag\", \"owner\"]")
        ),
    )
    .unwrap();
    let core = fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
    fs::write(
        root.join("core/Cargo.toml"),
        format!("{core}\n[features]\nextra = []\n"),
    )
    .unwrap();
    let tool = fs::read_to_string(root.join("tool/Cargo.toml")).unwrap();
    fs::write(
        root.join("tool/Cargo.toml"),
        tool.replace(", optional = true", ""),
    )
    .unwrap();
    fs::write(
        root.join("tool/src/main.rs"),
        "fn main() {\n    println!(\"{} {}\", renamed::extra(), cfg!(debug_assertions));\n}\n",
    )
    .unwrap();
    // A selected library-only package enables a feature exclusively through dev-dependencies.
    fs::create_dir_all(root.join("owner/src")).unwrap();
    fs::write(root.join("owner/Cargo.toml"), "[package]\nname = \"feature-owner\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dev-dependencies]\nmini-core = { path = \"../core\", features = [\"extra\"] }\n").unwrap();
    fs::write(
        root.join("owner/src/lib.rs"),
        r#"#[test]
fn peer_keeps_dev_features() {
    assert!(mini_core::extra());
    assert!(!cfg!(debug_assertions));
    let peer = std::env::var_os("CARGO_BIN_EXE_mini-tool").expect("peer binary");
    let output = std::process::Command::new(peer).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"true false\n");
}
"#,
    )
    .unwrap();
    fs::write(root.join("core/src/lib.rs"), "pub fn value() -> u32 {\n    1\n}\npub fn extra() -> bool {\n    cfg!(feature = \"extra\")\n}\n").unwrap();
    ok(&root, "cargo", &["generate-lockfile", "--offline"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "dev feature fixture"]);
    let core = fs::read_to_string(root.join("core/src/lib.rs")).unwrap();
    fs::write(root.join("core/src/lib.rs"), core.replace('1', "2")).unwrap();
    let output = ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--json", "--no-compiler-cache"],
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["affected"],
        serde_json::json!(["feature-owner", "mini-core", "mini-tool"])
    );
    let rows = report["checks"].as_array().unwrap();
    assert!(!rows.iter().any(|row| row["check"] == "test-binaries"));
    let run = fs::read_dir(root.join("target/ara-dev"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("test-build.json").is_file())
        .unwrap();
    let messages: Vec<serde_json::Value> = fs::read_to_string(run.join("test-build.json"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(messages.iter().any(|message| {
        message["target"]["name"] == "mini-tool"
            && message["profile"]["test"] == false
            && message["executable"].is_string()
    }));
    let regular_core: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "mini_core"
                && message["profile"]["test"] == false
        })
        .collect();
    assert_eq!(regular_core.len(), 1);
    assert_eq!(regular_core[0]["features"], serde_json::json!(["extra"]));
    assert!(
        !fs::read_to_string(run.join("tests.log"))
            .unwrap()
            .contains("Compiling "),
        "executing prepared tests must reuse the compiled feature union"
    );
}

#[test]
fn managed_changes_select_the_script_host_and_consumers_with_a_conservative_fallback() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        manifest.replace("\"diag\"]", "\"diag\", \"script\"]"),
    )
    .unwrap();
    fs::create_dir_all(root.join("script/src")).unwrap();
    fs::write(
        root.join("script/Cargo.toml"),
        "[package]\nname = \"ara-script\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        root.join("script/src/lib.rs"),
        "pub fn value() -> u32 {\n    1\n}\n",
    )
    .unwrap();
    let tool = fs::read_to_string(root.join("tool/Cargo.toml")).unwrap();
    fs::write(root.join("tool/Cargo.toml"), format!("{tool}\n[dev-dependencies]\nscript = {{ package = \"ara-script\", path = \"../script\" }}\n")).unwrap();
    ok(&root, "cargo", &["generate-lockfile", "--offline"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "script host"]);
    for (root, expected) in [
        (root, serde_json::json!(["ara-script", "mini-tool"])),
        (
            fixture(),
            serde_json::json!(["ara-diag", "mini-core", "mini-tool"]),
        ),
    ] {
        fs::create_dir_all(root.join("dotnet/src")).unwrap();
        fs::write(
            root.join("dotnet/src/Managed.cs"),
            "// Managed ABI change\n",
        )
        .unwrap();
        let output = ok(
            &root,
            env!("CARGO_BIN_EXE_ara-dev"),
            &["check", "--plan", "--json"],
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["affected"], expected);
    }
}

#[test]
fn compiler_cache_bypass_preserves_clippy_and_records_total_time() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::create_dir_all(root.join(".cargo")).unwrap();
    fs::write(
        root.join(".cargo/config.toml"),
        "[build]\nrustc-wrapper = \"missing-compiler-cache-for-test\"\n",
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_ara-dev");
    assert!(
        !configured_command(&root, binary, &["check", "--json"])
            .env_remove("RUSTC_WRAPPER")
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = ok(&root, binary, &["check", "--json", "--no-compiler-cache"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["compiler_cache_bypassed"], true);
    assert!(report["elapsed_seconds"].as_f64().unwrap() > 0.0);
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    return 2;\n}\n",
    )
    .unwrap();
    let output = command(&root, binary, &["check", "--json", "--no-compiler-cache"]);
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["checks"].as_array().unwrap().iter().any(|row| {
        row["check"] == "clippy" && row["passed"] == false && row["timed_out"] == false
    }));
}

#[test]
fn known_failures_are_opt_in_and_do_not_hide_other_tests_in_the_same_target() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::create_dir_all(root.join("core/tests")).unwrap();
    fs::write(
        root.join("core/tests/scene.rs"),
        "#[test]\nfn known() {\n    panic!(\"upstream failure\");\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("known.json"),
        r#"[{"package":"mini-core","target":"scene","test":"known","output_contains":"upstream failure","reason":"Owned upstream"}]"#,
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_ara-dev");
    let output = command(&root, binary, &["check", "--json"]);
    assert!(!output.status.success());
    let output = ok(
        &root,
        binary,
        &["check", "--json", "--known-failures", "known.json"],
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["known_failures"][0]["test"], "known");
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["check"] == "tests" && row["known_failure"] == true)
    );
    fs::write(root.join("core/tests/scene.rs"), "#[test]\nfn known() {\n    panic!(\"upstream failure\");\n}\n#[test]\nfn unexpected() {\n    panic!(\"new regression\");\n}\n").unwrap();
    let output = command(
        &root,
        binary,
        &["check", "--json", "--known-failures", "known.json"],
    );
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], false);
    assert_eq!(report["known_failures"][0]["test"], "known");
}

#[test]
fn plan_keeps_test_edits_local_and_ignores_equivalent_workspace_moves() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::create_dir_all(root.join("core/tests")).unwrap();
    fs::write(
        root.join("core/tests/check.rs"),
        "#[test]\nfn checks_value() {\n    assert_eq!(mini_core::value(), 1);\n}\n",
    )
    .unwrap();
    fs::write(root.join("core/AGENTS.md"), "# Core\nDocumentation only.\n").unwrap();
    let output = ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--plan", "--json"],
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["planned"], true);
    assert!(
        report.get("passed").is_none(),
        "a plan must not claim gate success"
    );
    assert_eq!(report["affected"], serde_json::json!(["mini-core"]));
    assert_eq!(report["test_only"], serde_json::json!(["mini-core"]));

    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "test and docs"]);
    let root_manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("Cargo.toml"), format!("{root_manifest}\n[workspace.dependencies]\nrenamed = {{ package = \"mini-core\", path = \"core\" }}\n")).unwrap();
    let tool = fs::read_to_string(root.join("tool/Cargo.toml")).unwrap();
    fs::write(
        root.join("tool/Cargo.toml"),
        tool.replace(
            "{ package = \"mini-core\", path = \"../core\", optional = true }",
            "{ workspace = true, optional = true }",
        ),
    )
    .unwrap();
    let output = ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--plan", "--json"],
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["affected"], serde_json::json!([]));
}

#[test]
fn adding_a_workspace_package_does_not_invalidate_existing_packages() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::create_dir_all(root.join("extra/src")).unwrap();
    fs::write(
        root.join("extra/src/lib.rs"),
        "pub fn value() -> u32 {\n    1\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("extra/Cargo.toml"),
        "[package]\nname = \"extra\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        manifest.replace("\"diag\"]", "\"diag\", \"extra\"]"),
    )
    .unwrap();
    ok(&root, "cargo", &["generate-lockfile", "--offline"]);
    let output = ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--plan", "--json"],
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["affected"], serde_json::json!(["extra"]));
}

#[test]
fn gate_deadline_includes_metadata_and_compilation() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::write(
        root.join("core/build.rs"),
        "fn main() {\n    std::thread::sleep(std::time::Duration::from_secs(3));\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let output = command(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["check", "--json", "--timeout", "1"],
    );
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], false);
    assert!(String::from_utf8_lossy(&output.stdout).contains("timed out"));
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
}

#[test]
fn merge_commits_only_after_gate_and_stops_without_committing_other_conflicts() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    git(&root, &["checkout", "-b", "feature"]);
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "feature"]);
    git(&root, &["checkout", "main"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "feature"]);
    assert_eq!(
        String::from_utf8(git(&root, &["show", "-s", "--format=%P"]).stdout)
            .unwrap()
            .split_whitespace()
            .count(),
        2
    );
    assert!(git(&root, &["status", "--porcelain"]).stdout.is_empty());
    git(&root, &["checkout", "-b", "conflict"]);
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "theirs @generated"),
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    3\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "theirs"]);
    git(&root, &["checkout", "main"]);
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "ours @generated"),
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    4\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "ours"]);
    let before = git(&root, &["rev-parse", "HEAD"]).stdout;
    let output = command(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "conflict"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("core/src/lib.rs"));
    assert!(
        !fs::read_to_string(root.join("Cargo.lock"))
            .unwrap()
            .contains("<<<<<<<")
    );
    assert_eq!(
        git(&root, &["diff", "--name-only", "--diff-filter=U"]).stdout,
        b"core/src/lib.rs\n"
    );
    assert_eq!(before, git(&root, &["rev-parse", "HEAD"]).stdout);
    git(&root, &["merge", "--abort"]);
}

#[test]
fn lockfile_only_conflict_is_regenerated_without_upgrading_dependencies() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    git(&root, &["checkout", "-b", "feature"]);
    // Both sides edit the same generated header; their manifests stay compatible.
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "feature @generated"),
    )
    .unwrap();
    git(&root, &["commit", "-am", "theirs lock"]);
    git(&root, &["checkout", "main"]);
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "main @generated"),
    )
    .unwrap();
    git(&root, &["commit", "-am", "ours lock"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "feature"]);
    let actual = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    assert!(!actual.contains("<<<<<<<"));
    assert_eq!(actual.matches("version = \"0.1.0\"").count(), 3);
    assert!(git(&root, &["status", "--porcelain"]).stdout.is_empty());
}

#[test]
fn cleanup_keeps_current_dirty_unmerged_and_active_targets() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    let other = scratch("worktree");
    git(
        &root,
        &["worktree", "add", "-b", "merged", other.to_str().unwrap()],
    );
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/keep"), "current").unwrap();
    fs::create_dir_all(other.join("target/debug")).unwrap();
    fs::write(other.join("target/debug/cache"), "cache").unwrap();
    fs::write(other.join("dirty"), "untracked").unwrap();
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(other.join("target").exists(), "dirty tree was deleted");
    fs::remove_file(other.join("dirty")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(other.join("target/debug/.cargo-lock"))
        .unwrap();
    lock.lock().unwrap();
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(
        other.join("target").exists(),
        "active Cargo target was deleted"
    );
    drop(lock);
    // A clean but unmerged branch must also be kept.
    fs::write(
        other.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    git(&other, &["commit", "-am", "unmerged"]);
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(other.join("target").exists(), "unmerged target was deleted");
    git(&root, &["merge", "--ff-only", "merged"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["clean-worktrees"]);
    assert!(
        other.join("target").exists(),
        "default must be dry-run and fresh targets kept"
    );
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(!other.join("target").exists(), "eligible target was kept");
    assert!(
        root.join("target/keep").exists(),
        "current target was deleted"
    );
}
