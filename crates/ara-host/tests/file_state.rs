//! Shared snapshot adapter used by source edits and media path commands.
use ara_doc::{
    AssetId, Op,
    file_state::{self, FileState},
};
use ara_host::file_state::FileHistory;
use std::{fs, path::Path};

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn every_observed_filesystem_alias_has_one_history_key() {
    let root = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/file-history-tests")
        })
        .join(format!("unicode-{}", std::process::id()));
    fs::create_dir_all(&root).expect("fixture");
    for (i, (a, b)) in [
        ("Σ", "σ"),
        ("σ", "ς"),
        ("I", "ı"),
        ("İ", "i"),
        ("ß", "ẞ"),
        ("ß", "SS"),
        ("K", "K"),
        ("é", "e\u{301}"),
        ("𐐀", "𐐨"),
    ]
    .into_iter()
    .enumerate()
    {
        let a = format!("{i}-{a}.bin");
        let b = format!("{i}-{b}.bin");
        fs::write(root.join(&a), b"base").expect("source");
        let aliases = fs::read(root.join(&b)).is_ok();
        assert!(
            !aliases || file_state::same_path(&a, &b),
            "filesystem alias {a}/{b} must share a key"
        );
        assert_eq!(
            file_state::same_path(&a, &b),
            file_state::path_key(&a) == file_state::path_key(&b)
        );
    }
    let store = FileHistory::new(&root);
    fs::write(root.join("Σ.bin"), b"base").expect("source");
    let before = store.snapshot("Σ.bin").expect("snapshot").expect("exists");
    let after = store.put(b"after").expect("blob");
    let op = |path: &str, prev, state| Op::File {
        doc: AssetId::parse("sc_alias").expect("id"),
        id: file_state::record_id(path),
        path: path.into(),
        prev,
        state,
    };
    let ops = [
        op("Σ.bin", Some(before.clone()), None),
        op("ς.bin", None, Some(after)),
    ];
    store
        .materialize(&ops)
        .expect("case-only move shares the staged absence");
    store
        .materialize(&ops.iter().rev().map(Op::inverse).collect::<Vec<_>>())
        .expect("undo");
    assert_eq!(store.read(&before).expect("original snapshot"), b"base");
    assert_eq!(fs::read(root.join("Σ.bin")).expect("restored"), b"base");
}

#[cfg(unix)]
#[test]
fn a_symlink_introduced_after_preparation_cannot_redirect_restore() {
    let base = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/file-history-tests")
        })
        .join(format!("symlink-{}", std::process::id()));
    let root = base.join("project");
    let outside = base.join("outside");
    fs::create_dir_all(root.join("media")).expect("source");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(root.join("media/file.bin"), b"base").expect("source");
    fs::write(outside.join("file.bin"), b"base").expect("outside");
    let store = FileHistory::new(&root);
    let prepared = store.prepare_tree("media").expect("prepare");
    let replacement = store.put(b"replacement").expect("blob");
    fs::remove_file(root.join("media/file.bin")).expect("source");
    fs::remove_dir(root.join("media")).expect("empty directory");
    std::os::unix::fs::symlink(&outside, root.join("media")).expect("symlink");
    assert_eq!(
        store
            .revalidate(&prepared)
            .expect_err("revalidate refuses link")
            .code,
        ara_diag::codes::IO
    );
    assert_eq!(
        store
            .restore("media/file.bin", Some(&replacement))
            .expect_err("restore refuses link")
            .code,
        ara_diag::codes::IO
    );
    assert_eq!(
        fs::read(outside.join("file.bin")).expect("outside bytes"),
        b"base"
    );
    fs::remove_file(root.join("media")).expect("unlink fixture only");
}

#[cfg(windows)]
#[test]
fn a_junction_introduced_after_preparation_cannot_redirect_restore() {
    use std::os::windows::process::CommandExt;
    let base = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/file-history-tests")
        })
        .join(format!("junction-{}", std::process::id()));
    let root = base.join("project");
    let outside = base.join("outside");
    fs::create_dir_all(root.join("media")).expect("source directory");
    fs::create_dir_all(&outside).expect("outside directory");
    fs::write(root.join("media/file.bin"), b"base").expect("source");
    fs::write(outside.join("file.bin"), b"base").expect("same bytes outside");
    let store = FileHistory::new(&root);
    let prepared = store
        .prepare_tree("media")
        .expect("prepare ordinary directory");
    let replacement = store.put(b"replacement").expect("blob");
    fs::remove_file(root.join("media/file.bin")).expect("remove source");
    fs::remove_dir(root.join("media")).expect("remove empty source directory");
    // mklink reads any `/` in a path as a switch, so pass normalized absolute paths.
    let result = std::process::Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(std::path::absolute(root.join("media")).expect("junction path"))
        .arg(std::path::absolute(&outside).expect("junction target"))
        .creation_flags(0x0800_0000)
        .output()
        .expect("create junction");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    // Remove the junction itself before any assertion, never its destination.
    let revalidated = store.revalidate(&prepared);
    let restored = store.restore("media/file.bin", Some(&replacement));
    let restored_target = store.restore("media", Some(&FileState::directory()));
    fs::remove_dir(root.join("media")).expect("remove junction only");
    assert_eq!(
        fs::read(outside.join("file.bin")).expect("outside bytes"),
        b"base"
    );
    for result in [revalidated, restored, restored_target] {
        let error = result.expect_err("junction must be refused at preparation and restore");
        assert_eq!(error.code, ara_diag::codes::IO);
        assert!(error.message.contains("reparse"));
    }
}

#[test]
fn prepared_inventory_revalidates_directory_membership_and_opaque_owner_metadata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-history-tests")
        .join(format!("prepared-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("media/empty")).expect("folder");
    fs::write(root.join("media/model.bin"), b"base").expect("file");
    let history = FileHistory::new(&root);
    let mut prepared = history
        .prepare_tree("media")
        .expect("complete prepared inventory");
    prepared.metadata = serde_json::json!({"worker":"v7", "policyHash":"policy-before"});
    history
        .revalidate_with(&prepared, |metadata| {
            assert_eq!(metadata["worker"], "v7");
            Ok(())
        })
        .expect("owner metadata passes through unchanged");
    fs::write(root.join("media/extra.bin"), b"unexpected").expect("concurrent addition");
    let error = history
        .revalidate(&prepared)
        .expect_err("a newly added entry invalidates a complete inventory");
    assert_eq!(error.code, ara_diag::codes::STALE_BASE);
    fs::remove_file(root.join("media/extra.bin")).expect("remove fixture extra");
    let error = history
        .revalidate_with(&prepared, |_| {
            Err(ara_diag::Diagnostic::error(
                ara_diag::codes::STALE_BASE,
                "worker/policy identity changed",
            ))
        })
        .expect_err("the asset owner can veto changed worker metadata");
    assert!(error.message.contains("worker/policy"));
}

#[test]
fn snapshots_keep_binary_bytes_empty_folders_and_preflight_all_paths_before_restore() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-history-tests")
        .join(format!("snapshots-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("media/empty")).expect("empty folder");
    fs::write(root.join("media/model.bin"), [0, 255, 10, 0, 20]).expect("binary");
    let store = FileHistory::new(&root);
    let tree = store.snapshot_tree("media").expect("snapshot tree");
    assert_eq!(
        tree.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        ["media", "media/empty", "media/model.bin"]
    );
    let state = tree[2].1.clone();
    assert_eq!(
        store.read(&state).expect("read by token"),
        [0, 255, 10, 0, 20]
    );
    assert_eq!(
        store.put(&[0, 255, 10, 0, 20]).expect("deduplicated"),
        state
    );
    let doc = AssetId::parse("sc_snapshot").expect("anchor");
    let transition = |path: &str, prev, state| Op::File {
        doc: doc.clone(),
        id: file_state::record_id(path),
        path: path.into(),
        prev,
        state,
    };
    let ops = vec![
        transition("media/copy.bin", None, Some(state.clone())),
        transition("media/model.bin", Some(state.clone()), None),
    ];
    fs::write(root.join("media/model.bin"), b"external").expect("external edit");
    assert!(store.materialize(&ops).is_err());
    assert!(
        !root.join("media/copy.bin").exists(),
        "preflight failure must leave earlier paths unchanged"
    );
    store
        .restore("media/model.bin", Some(&state))
        .expect("restore snapshot");
    store.materialize(&ops).expect("move bytes");
    assert!(!root.join("media/model.bin").exists());
    let inverse = ops.iter().rev().map(Op::inverse).collect::<Vec<_>>();
    store.materialize(&inverse).expect("inverse");
    assert_eq!(
        fs::read(root.join("media/model.bin")).expect("original"),
        [0, 255, 10, 0, 20]
    );
    assert!(!root.join("media/copy.bin").exists());
    assert!(root.join("media/empty").is_dir());
    let bad = FileState {
        hash: "../outside".into(),
        ..state
    };
    assert!(store.read(&bad).is_err());
}
