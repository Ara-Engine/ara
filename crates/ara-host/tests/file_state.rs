//! Shared snapshot adapter used by source edits and media path commands.
use ara_doc::{
    AssetId, Op,
    file_state::{self, FileState},
};
use ara_host::file_state::FileHistory;
use std::{fs, path::Path};

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
