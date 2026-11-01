use super::*;
use std::fs;
use std::time::Duration;

fn crate_at(root: &Path, name: &str, dependencies: &str) {
    let dir = root.join("crates").join(name);
    fs::create_dir_all(dir.join("src/nested")).expect("create crate");
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\n[dependencies]\n{dependencies}"),
    )
    .expect("manifest");
    fs::write(dir.join("src/lib.rs"), "").expect("source");
    fs::write(dir.join("src/nested/mod.rs"), "").expect("nested source");
}

fn touch(path: &Path, at: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(at)
        .expect("set mtime");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// A tree where `ara-player` depends on `ara-render` (by path), `ara-core` (through the
/// workspace table) and an optional crate, with every file older than the returned
/// build time.
fn tree() -> (tempfile::TempDir, SystemTime) {
    let root = tempfile::tempdir().expect("tempdir");
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\n[workspace.dependencies]\nara-core = { path = \"crates/ara-core\" }\n",
    )
    .expect("workspace");
    crate_at(
        root.path(),
        "ara-player",
        "ara-render = { path = \"../ara-render\" }\nara-core.workspace = true\n\
         ara-lab = { path = \"../ara-lab\", optional = true }\n",
    );
    crate_at(root.path(), "ara-render", "");
    crate_at(root.path(), "ara-core", "");
    crate_at(root.path(), "ara-lab", "");
    let built = SystemTime::now();
    let old = built - Duration::from_secs(60);
    for file in walk(root.path()) {
        touch(&file, old);
    }
    (root, built)
}

#[test]
fn a_fresh_tree_passes() {
    let (root, built) = tree();
    assert_eq!(assess(root.path(), "ara-player", Some(built)), None);
}

#[test]
fn a_changed_dependency_source_makes_the_companion_stale() {
    let (root, built) = tree();
    for (krate, file) in [
        ("ara-render", "src/nested/mod.rs"),
        ("ara-core", "Cargo.toml"),
        ("ara-player", "src/lib.rs"),
    ] {
        let path = root.path().join("crates").join(krate).join(file);
        touch(&path, built + Duration::from_secs(5));
        assert_eq!(
            assess(root.path(), "ara-player", Some(built)),
            Some(Reason::Stale {
                newer: path.clone()
            }),
            "{krate}"
        );
        touch(&path, built - Duration::from_secs(60));
    }
}

#[test]
fn optional_dependencies_do_not_count() {
    let (root, built) = tree();
    touch(
        &root.path().join("crates/ara-lab/src/lib.rs"),
        built + Duration::from_secs(5),
    );
    assert_eq!(assess(root.path(), "ara-player", Some(built)), None);
}

#[test]
fn a_missing_executable_is_refused() {
    let (root, _) = tree();
    assert_eq!(
        assess(root.path(), "ara-player", None),
        Some(Reason::Missing)
    );
}

#[test]
fn the_message_names_the_binary_and_the_fix() {
    let stale = Refusal {
        exe: PathBuf::from("target/release/ara-player.exe"),
        package: "ara-player",
        reason: Reason::Stale {
            newer: PathBuf::from("crates/ara-render/src/lib.rs"),
        },
    };
    let text = stale.to_string();
    assert!(text.contains("ara-player.exe") && text.contains("crates/ara-render/src/lib.rs"));
    assert!(text.contains("cargo build -p ara-player --release"));
    let perf = Refusal {
        exe: PathBuf::from("target/perf/ara-import-worker"),
        package: "ara-import-worker",
        reason: Reason::Missing,
    };
    assert!(perf.fix().ends_with("-p ara-import-worker --profile perf"));
}

#[test]
fn only_companions_beside_ara_are_checked() {
    enforce();
    assert_eq!(verify(Path::new("target/debug/ara-editor")), Ok(()));
    assert_eq!(verify(Path::new("elsewhere/ara-player")), Ok(()));
}

#[test]
fn test_only_modules_do_not_make_a_companion_stale() {
    let (root, built) = tree();
    for file in [
        "src/tests.rs",
        "src/quality_tests/row.rs",
        "src/post_tests.rs",
    ] {
        let path = root.path().join("crates/ara-render").join(file);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(&path, "").expect("file");
        touch(&path, built + Duration::from_secs(5));
    }
    assert_eq!(assess(root.path(), "ara-player", Some(built)), None);
}
