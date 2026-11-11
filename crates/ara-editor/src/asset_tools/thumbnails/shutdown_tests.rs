//! Close bounded replies before joining the worker, and discard queued work on stop.
#![allow(clippy::unwrap_used)]
use super::*;
use std::time::Duration;

#[test]
fn drop_disconnects_a_blocked_result_sender_and_joins_the_worker() {
    let (send, requests) = mpsc::sync_channel::<Request>(1);
    let (results, receive) = mpsc::sync_channel(0);
    let (sending, sending_started) = mpsc::channel();
    let (finished, completions) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sending.send(()).unwrap();
        // A zero-capacity reply channel cannot complete this send until the UI
        // consumes the reply or disconnects. Shutdown must disconnect it first.
        assert!(
            results
                .send(Done::Pending("shutdown-probe".into()))
                .is_err()
        );
        assert!(requests.recv().is_err());
        finished.send(()).unwrap();
    });
    let thumbnails = Thumbnails {
        send: Some(send),
        receive: Some(receive),
        stopping: Arc::new(AtomicBool::new(false)),
        worker: Some(worker),
        requested: BTreeSet::new(),
        pending: BTreeSet::new(),
    };
    sending_started
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    let (dropped, drop_finished) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        drop(thumbnails);
        dropped.send(()).unwrap();
    });
    drop_finished
        .recv_timeout(Duration::from_secs(3))
        .expect("Joining while a bounded reply sender remains connected deadlocks shutdown");
    owner.join().unwrap();
    assert!(
        completions.try_recv().is_ok(),
        "The worker must finish before the owner returns"
    );
}
