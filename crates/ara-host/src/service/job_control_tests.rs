//! Existing subscriptions observe service-owned cancellation gates without authoring.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::{Host, HostConfig, Project};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

enum Action {
    Start,
    Set(String, bool),
    Cancel(String),
    Finish(String, bool),
}
struct Probe {
    input: mpsc::Receiver<Action>,
    output: mpsc::Sender<Value>,
}
impl Service for Probe {
    fn name(&self) -> &'static str {
        "job-control-probe"
    }
    fn methods(&self, _: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }
    fn call(&mut self, _: &mut Cx<'_>, _: &str, _: Value) -> Result<Value, Vec<Diagnostic>> {
        unreachable!("The test probe has no callable methods")
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        while let Ok(action) = self.input.try_recv() {
            let (job, cancelled) = match action {
                Action::Start => {
                    let job = cx.start_job("Cancellation gate fixture");
                    cx.job_progress(&job, 0.375, "Preparing measured input");
                    (job, None)
                }
                Action::Set(job, cancellable) => {
                    cx.set_job_cancellable(&job, cancellable);
                    (job, None)
                }
                Action::Cancel(job) => {
                    let cancelled = cx.cancel_job(&job);
                    (job, Some(cancelled))
                }
                Action::Finish(job, success) => {
                    cx.finish_job(
                        &job,
                        if success {
                            Ok(json!({"ok":true}))
                        } else {
                            Err(vec![Diagnostic::error(
                                ara_diag::codes::INVALID_PARAMS,
                                "fixture failure",
                            )])
                        },
                    );
                    (job, None)
                }
            };
            self.output
                .send(json!({"job":job,"record":cx.job_record(&job),"cancelled":cancelled}))
                .unwrap();
        }
    }
}

const SUBSCRIBER: ConnId = ConnId(81);
struct Harness {
    host: Option<Host>,
    input: mpsc::Sender<Action>,
    output: mpsc::Receiver<Value>,
    root: PathBuf,
    target: PathBuf,
    revision: u64,
    history: usize,
}
impl Harness {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let target = std::path::absolute(target).unwrap();
        let root = target.join("host-job-control").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(&json!({
                "name":"Job cancellation fixture","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION
            })),
        )
        .unwrap();
        let mut host = Host::open(
            Project::open(&root).unwrap(),
            HostConfig {
                seed: Some(81),
                watch_every: Duration::from_secs(3600),
                ..Default::default()
            },
        )
        .unwrap();
        let (input, receive) = mpsc::channel();
        let (send, output) = mpsc::channel();
        host.register(Box::new(Probe {
            input: receive,
            output: send,
        }))
        .unwrap();
        let session = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"author":"human:job-control-test","client":"test"}),
            )
            .unwrap()["session"]
            .clone();
        host.call(
            SUBSCRIBER,
            "subscribe",
            json!({"session":session,"topics":["jobs","ops","history"]}),
        )
        .unwrap();
        host.drain_outbox();
        let (revision, history) = (host.store().revision(), host.store().history().len());
        Self {
            host: Some(host),
            input,
            output,
            root,
            target,
            revision,
            history,
        }
    }
    fn act(&mut self, action: Action) -> (Value, Vec<Value>) {
        self.input.send(action).unwrap();
        let host = self.host.as_mut().unwrap();
        host.tick();
        let result = self.output.try_recv().unwrap();
        let notes = host
            .drain_outbox()
            .into_iter()
            .filter(|(conn, _)| *conn == SUBSCRIBER)
            .map(|(_, note)| note)
            .collect::<Vec<_>>();
        assert_eq!(
            host.store().revision(),
            self.revision,
            "job control must not author document ops"
        );
        assert_eq!(
            host.store().history().len(),
            self.history,
            "job control must not add history"
        );
        assert!(
            notes.iter().all(|note| note["method"] == "jobs"),
            "no ops/history notification is allowed"
        );
        (result, notes)
    }
    fn start(&mut self) -> String {
        self.act(Action::Start).0["job"].as_str().unwrap().into()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.host.take());
        assert!(self.root.starts_with(&self.target));
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn notified(notes: &[Value], record: &Value) {
    assert_eq!(
        notes.len(),
        1,
        "exactly one existing Jobs notification per state change"
    );
    assert_eq!(notes[0]["method"], "jobs");
    assert_eq!(&notes[0]["params"], record);
}

#[test]
fn each_actual_gate_flip_notifies_once_without_rewriting_progress_or_message() {
    let mut host = Harness::new();
    let job = host.start();
    let (same, notes) = host.act(Action::Set(job.clone(), true));
    assert!(notes.is_empty());
    for cancellable in [false, true] {
        let (result, notes) = host.act(Action::Set(job.clone(), cancellable));
        notified(&notes, &result["record"]);
        assert_eq!(result["record"]["cancellable"], cancellable);
        let mut expected = same["record"].clone();
        expected["cancellable"] = cancellable.into();
        assert_eq!(
            result["record"], expected,
            "only the cancellation gate changes"
        );
        let (repeated, notes) = host.act(Action::Set(job.clone(), cancellable));
        assert!(notes.is_empty());
        assert_eq!(repeated["record"], result["record"]);
    }
}

#[test]
fn noncancellable_cancel_is_unchanged_then_one_successful_cancel_notifies_once() {
    let mut host = Harness::new();
    let job = host.start();
    let (disabled, notes) = host.act(Action::Set(job.clone(), false));
    notified(&notes, &disabled["record"]);
    let (blocked, notes) = host.act(Action::Cancel(job.clone()));
    assert_eq!(blocked["cancelled"], false);
    assert_eq!(blocked["record"], disabled["record"]);
    assert!(notes.is_empty());
    let (enabled, notes) = host.act(Action::Set(job.clone(), true));
    notified(&notes, &enabled["record"]);
    let (cancelled, notes) = host.act(Action::Cancel(job.clone()));
    notified(&notes, &cancelled["record"]);
    assert_eq!(cancelled["cancelled"], true);
    let mut expected = enabled["record"].clone();
    expected["status"] = "cancelled".into();
    expected["cancellable"] = false.into();
    assert_eq!(cancelled["record"], expected);
    let (again, notes) = host.act(Action::Cancel(job.clone()));
    assert_eq!(again["cancelled"], false);
    assert_eq!(again["record"], expected);
    assert!(notes.is_empty());
    let (reopen, notes) = host.act(Action::Set(job, true));
    assert_eq!(reopen["record"], expected);
    assert!(notes.is_empty());
}

#[test]
fn unknown_and_finished_jobs_never_flip_cancel_or_publish_another_notification() {
    let mut host = Harness::new();
    for cancellable in [false, true] {
        let (missing, notes) = host.act(Action::Set("jb_missing81".into(), cancellable));
        assert_eq!(missing["record"], Value::Null);
        assert!(notes.is_empty());
    }
    let (missing, notes) = host.act(Action::Cancel("jb_missing81".into()));
    assert_eq!(missing["cancelled"], false);
    assert!(notes.is_empty());
    for success in [true, false] {
        let job = host.start();
        let (finished, notes) = host.act(Action::Finish(job.clone(), success));
        notified(&notes, &finished["record"]);
        for cancellable in [false, true] {
            let (unchanged, notes) = host.act(Action::Set(job.clone(), cancellable));
            assert_eq!(unchanged["record"], finished["record"]);
            assert!(notes.is_empty());
        }
        let (cancelled, notes) = host.act(Action::Cancel(job));
        assert_eq!(cancelled["cancelled"], false);
        assert_eq!(cancelled["record"], finished["record"]);
        assert!(notes.is_empty());
    }
}
