//! Synchronize a real active timeout without sleeps or simulated stall counts.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn final_storage_wait_has_no_monitor_and_preserves_an_active_loop_stall() {
    let root = crate::test_target().join("monkey-watchdog").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (detected, notice) = mpsc::channel();
    let mut watchdog = Watchdog::spawn(root.clone(), Duration::from_millis(1), move || {
        let _ = detected.send(());
    })
    .unwrap();
    // The auxiliary notification follows the real watchdog's queued count. Leave
    // that count unread so finish must collect it after joining the active worker.
    notice.recv_timeout(Duration::from_secs(5)).unwrap();
    watchdog.finish();
    assert_eq!(watchdog.count(), 1);
    assert!(watchdog.worker.is_none());
    assert!(
        watchdog
            .sender
            .send(Message::Progress {
                step: 1,
                action: None
            })
            .is_err()
    );
    // A final report writer can remain blocked on storage with no live heartbeat
    // monitor. Repeated shutdown cannot clear the already observed real stall.
    watchdog.finish();
    assert_eq!(watchdog.count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
