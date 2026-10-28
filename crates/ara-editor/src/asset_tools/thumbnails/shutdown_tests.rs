//! Close bounded replies before joining workers, and discard queued work on stop.
#![allow(clippy::unwrap_used)]
use super::*;
use std::time::Duration;

#[test]
fn drop_disconnects_a_blocked_result_sender_and_joins_both_workers() {
    let (send, requests) = mpsc::sync_channel::<Request>(1);
    let (previews, preview_requests) = mpsc::sync_channel::<Request>(1);
    let (results, receive) = mpsc::sync_channel(0);
    let (sending, sending_started) = mpsc::channel();
    let (finished, completions) = mpsc::channel();
    let second_finished = finished.clone();
    let first = std::thread::spawn(move || {
        sending.send(()).unwrap();
        // A zero-capacity reply channel cannot complete this send until the UI
        // consumes the reply or disconnects. Shutdown must disconnect it first.
        assert!(
            results
                .send(Done::Pending("shutdown-probe".into()))
                .is_err()
        );
        assert!(requests.recv().is_err());
        finished.send(0).unwrap();
    });
    let second = std::thread::spawn(move || {
        assert!(preview_requests.recv().is_err());
        second_finished.send(1).unwrap();
    });
    let thumbnails = Thumbnails {
        send: Some(send),
        previews: Some(previews),
        receive: Some(receive),
        stopping: Arc::new(AtomicBool::new(false)),
        workers: [Some(first), Some(second)],
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
    let completed: BTreeSet<_> = completions.try_iter().collect();
    assert_eq!(
        completed,
        BTreeSet::from([0, 1]),
        "Both workers must finish before the owner returns"
    );
}

#[test]
fn an_already_stopped_real_worker_discards_queued_source_work() {
    let (send, requests) = mpsc::sync_channel(1);
    let (results, receive) = mpsc::sync_channel(1);
    assert!(
        send.send(Request {
            path: "queued-file-that-does-not-exist.png".into(),
            key: "cancelled-queue".into(),
            source: Source::File,
            shot: Shot::surface(false),
            pixels: TILE,
        })
        .is_ok()
    );
    let worker = worker(
        crate::test_target(),
        requests,
        results,
        Arc::new(AtomicBool::new(true)),
    );
    worker.join().unwrap();
    assert!(
        receive.try_recv().is_err(),
        "Stopping discards queued work rather than finishing its source read"
    );
}
