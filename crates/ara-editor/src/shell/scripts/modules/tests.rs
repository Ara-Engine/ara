#![allow(clippy::unwrap_used)]
use super::*;
use std::sync::mpsc::{self, SyncSender};

#[path = "tests/retained.rs"]
mod retained;

fn blocked(list: &mut ModuleList, root: &Path) -> SyncSender<Inventory> {
    let (started, entered) = mpsc::sync_channel(1);
    let (release, result) = mpsc::sync_channel(1);
    assert!(list.start_with(root, move |_| {
        started.send(()).unwrap();
        result.recv().unwrap()
    }).unwrap());
    entered.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    release
}

fn finish(list: &mut ModuleList) -> Vec<ara_diag::Diagnostic> {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(result) = list.poll() {
            assert!(list.job.is_none());
            return result;
        }
        assert!(Instant::now() < deadline, "controlled discovery did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn one_blocked_worker_coalesces_requests_and_cancel_does_not_claim_retirement() {
    let mut list = ModuleList::default();
    let release = blocked(&mut list, Path::new("project"));
    list.intent = true;
    for _ in 0..32 {
        assert!(!list.start_with(Path::new("project"), |_| panic!("duplicate task")).unwrap());
        assert!(list.poll().is_none());
    }
    assert_eq!(list.started, 1);
    list.cancel_intent();
    assert!(!list.intent);
    assert!(list.job.is_some(), "cancellation is presentation-only");
    release.send(Ok(vec![PathBuf::from("project/module")])).unwrap();
    assert!(finish(&mut list).is_empty());
    assert!(!list.intent, "completion cannot reinstate a cancelled UI intent");
    assert_eq!(list.paths(), ["module"]);
    assert_eq!((list.started, list.completed, list.cancelled), (1, 1, 1));
}

#[test]
fn completed_error_keeps_last_good_and_successful_empty_inventory_replaces_it() {
    let mut list = ModuleList::default();
    let root = Path::new("project");
    let release = blocked(&mut list, root);
    release.send(Ok(vec![root.join("game"), root.join("tools")])).unwrap();
    assert!(finish(&mut list).is_empty());
    let good = list.paths.clone();
    let diagnostic = ara_diag::Diagnostic::error(
        ara_script::codes::UNKNOWN_MODULE,
        "combat references missing core",
    );
    let release = blocked(&mut list, root);
    release.send(Err(vec![diagnostic.clone()])).unwrap();
    assert_eq!(finish(&mut list), vec![diagnostic]);
    assert_eq!(list.paths, good);
    assert!(list.error.as_deref().unwrap().contains("missing core"));
    let release = blocked(&mut list, root);
    release.send(Ok(Vec::new())).unwrap();
    assert!(finish(&mut list).is_empty());
    assert!(list.paths.is_empty());
    assert!(list.error.is_none());
}
