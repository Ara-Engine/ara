#![allow(clippy::unwrap_used)]
//! The tests' scratch directories delete themselves: the valley tests left about 76 MB each in
//! `/tmp` on every run and filled the gate's quota. The guard removes its directory when the
//! test ends and when it fails.
mod support;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Mutex,
};
use support::{Scratch, demo_copy};

#[test]
fn a_scratch_directory_is_removed_when_its_test_ends_or_fails() {
    // Ends normally.
    let ended = {
        let dir = Scratch::new("scratch-ended");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/file"), b"x").unwrap();
        assert!(dir.join("a/b/file").exists());
        dir.to_path_buf()
    };
    assert!(!ended.exists(), "{} is still there", ended.display());

    // The test body panics: unwinding drops the guard.
    let failed: Mutex<Option<PathBuf>> = Mutex::new(None);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let dir = Scratch::new("scratch-failed");
        std::fs::create_dir_all(dir.join("c")).unwrap();
        *failed.lock().unwrap() = Some(dir.to_path_buf());
        panic!("a failing test");
    }));
    assert!(outcome.is_err());
    let failed = failed.into_inner().unwrap().unwrap();
    assert!(!failed.exists(), "{} is still there", failed.display());
}

/// The helper the play tests use: a copy of the demo project that is gone afterwards.
#[test]
fn a_demo_copy_is_removed_with_its_guard() {
    let path = {
        let dir = demo_copy("scratch-demo");
        assert!(dir.join("ara.project.json").exists(), "a copy of the demo");
        dir.to_path_buf()
    };
    assert!(!path.exists(), "{} is still there", path.display());
}
