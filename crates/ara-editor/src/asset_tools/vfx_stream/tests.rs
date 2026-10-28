//! The preview owner must wait for an in-flight task's teardown, without a GPU.
#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn drop_closes_requests_and_waits_for_the_current_owned_task_to_finish() {
    let (send, requests) = mpsc::channel();
    let (frames, receive) = mpsc::sync_channel(2);
    let (disconnecting, disconnected) = mpsc::channel();
    let (release, finish) = mpsc::channel();
    let (finished, task_finished) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        assert!(requests.recv().is_err());
        disconnecting.send(()).unwrap();
        // Represents a currently owned task's native-resource retirement. The
        // owner must not return until it has completed on this worker.
        finish.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(frames);
        finished.send(()).unwrap();
    });
    let stream = VfxStream {
        send: Some(send),
        receive: Some(receive),
        stopping: Arc::new(AtomicBool::new(false)),
        worker: Some(worker),
        identity: None,
        playing: false,
        rate: 100,
    };
    let (dropped, drop_finished) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        drop(stream);
        dropped.send(()).unwrap();
    });
    disconnected.recv_timeout(Duration::from_secs(3)).unwrap();
    let returned_early = drop_finished
        .recv_timeout(Duration::from_millis(40))
        .is_ok();
    release.send(()).unwrap();
    task_finished.recv_timeout(Duration::from_secs(3)).unwrap();
    if !returned_early {
        drop_finished.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    owner.join().unwrap();
    assert!(
        !returned_early,
        "Dropping the UI owner must join its native-resource worker"
    );
}
