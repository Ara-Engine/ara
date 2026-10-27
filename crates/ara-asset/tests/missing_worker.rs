//! A worker executable that cannot start (not built, a wrong `ARA_IMPORT_WORKER`)
//! fails every import with its own path, not a bare "file not found".
use ara_asset::ProcessWorker;

#[test]
fn a_missing_worker_names_its_executable() {
    let missing = std::env::temp_dir().join("ara-no-such-import-worker.exe");
    let error = ProcessWorker::new(&missing)
        .versions()
        .expect_err("a missing worker cannot answer")
        .to_string();
    assert!(
        error.contains("import worker") && error.contains("ara-no-such-import-worker"),
        "{error}"
    );
}
