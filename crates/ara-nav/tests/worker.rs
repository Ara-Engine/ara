#![allow(clippy::unwrap_used)]
mod common;
use ara_nav::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn process_worker_returns_prepared_mesh_and_retains_last_good_on_failure() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_ara-nav-worker"));
    let request = WorkerRequest {
        input: common::plane(12.0),
        previous: None,
        cache: None,
    };
    let mut job = BakeJob::spawn(worker.clone(), request.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (artifact, mesh) = loop {
        assert!(Instant::now() < deadline, "worker deadline");
        match job.poll() {
            JobPoll::Pending => std::thread::sleep(Duration::from_millis(10)),
            JobPoll::Ready(result) => break result.unwrap(),
            JobPoll::Finished => panic!("lost worker result"),
        }
    };
    assert_eq!(mesh.content_hash(), artifact.content_hash());
    assert!(matches!(job.poll(), JobPoll::Finished));
    let mut bad = request;
    bad.input.settings.cell_size = 0.0;
    let mut job = BakeJob::spawn(worker, bad).unwrap();
    loop {
        assert!(Instant::now() < deadline);
        match job.poll() {
            JobPoll::Pending => std::thread::sleep(Duration::from_millis(10)),
            JobPoll::Ready(result) => {
                assert_eq!(result.unwrap_err().kind, "invalidSettings");
                break;
            }
            JobPoll::Finished => panic!("lost error"),
        }
    }
    assert!(
        mesh.query()
            .unwrap()
            .path([2.0, 0.0, 2.0], [10.0, 0.0, 10.0], &Default::default())
            .unwrap()
            .complete
    );
}
