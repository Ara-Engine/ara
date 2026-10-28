//! Real Host jobs complete independently of authoring and panel revisions.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service};
use ara_schema::SchemaDb;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Outcome {
    Done,
    Failed,
    Cancelled,
}
enum Transition {
    Start,
    Finish(String, Outcome),
}
struct ControlledCompiler {
    input: mpsc::Receiver<Transition>,
    started: mpsc::Sender<String>,
}
impl Service for ControlledCompiler {
    fn name(&self) -> &'static str {
        "controlled-script-completion"
    }
    fn methods(&self, _: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }
    fn call(&mut self, _: &mut Cx<'_>, _: &str, _: Value) -> Result<Value, Vec<Diagnostic>> {
        unreachable!("The controlled compiler exposes no protocol methods")
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        while let Ok(transition) = self.input.try_recv() {
            match transition {
                Transition::Start => {
                    let job = cx.start_job("Author C# source");
                    cx.job_progress(&job, 0.6, "Compiling source");
                    self.started.send(job).unwrap();
                }
                Transition::Finish(job, Outcome::Cancelled) => {
                    assert!(cx.cancel_job(&job));
                }
                Transition::Finish(job, outcome) => cx.finish_job(
                    &job,
                    match outcome {
                        Outcome::Done => Ok(json!({"component":"Spinner"})),
                        Outcome::Failed => Err(vec![Diagnostic::error(
                            ara_diag::codes::INVALID_PARAMS,
                            "Controlled script compilation failed",
                        )]),
                        Outcome::Cancelled => unreachable!("handled above"),
                    },
                ),
            }
        }
    }
}

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("script-job-poll")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(&json!({
                "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Job polling","main":"levels/main.scene"
            })),
        )
        .unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(&json!({
                "$schema":"ara://schema/scene/1","id":"sc_jobpoll","records":{}
            })),
        )
        .unwrap();
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(editor: &mut Editor, shell: &mut Shell) -> (mpsc::Sender<Transition>, String) {
    let (input, receive) = mpsc::channel();
    let (send, started) = mpsc::channel();
    editor
        .host
        .register(Box::new(ControlledCompiler {
            input: receive,
            started: send,
        }))
        .unwrap();
    input.send(Transition::Start).unwrap();
    editor.tick();
    let job = started.try_recv().unwrap();
    shell.script_job = Some((job.clone(), Instant::now() - Duration::from_secs(1)));
    shell.rebuild_if_stale(editor).unwrap();
    shell.take_stats();
    (input, job)
}

#[test]
fn terminal_script_parent_retires_without_revision_changes_and_stays_idle() {
    for outcome in [Outcome::Done, Outcome::Failed, Outcome::Cancelled] {
        let project = Project::new();
        let mut editor = project.open();
        let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
        shell.offscreen = true;
        let (input, job) = start(&mut editor, &mut shell);
        let revision = editor.host.store().revision();
        let history = editor.host.store().history().len();
        let panels = editor.panels_revision;
        assert_eq!(shell.monkey_background_work()["scriptJob"]["job"], job);
        assert!(shell.script_job.is_some());

        input
            .send(Transition::Finish(job.clone(), outcome))
            .unwrap();
        editor.tick();
        assert_eq!(editor.host.store().revision(), revision);
        assert_eq!(editor.panels_revision, panels);
        shell.script_job.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(1);
        assert!(shell.rebuild_if_stale(&mut editor).unwrap());
        assert!(shell.script_job.is_none());
        assert_eq!(shell.take_stats().rebuilds, 1);
        assert_eq!(shell.monkey_background_work()["scriptJob"], Value::Null);
        assert_eq!(shell.monkey_background_work()["busy"], false);
        match outcome {
            Outcome::Done => assert_eq!(shell.status_note.as_deref(), Some("Spinner.cs compiled")),
            Outcome::Failed => assert_eq!(
                shell.status_note.as_deref(),
                Some("Controlled script compilation failed")
            ),
            Outcome::Cancelled => assert_eq!(
                shell.status_note.as_deref(),
                Some("Script compilation failed; see Problems")
            ),
        }
        for _ in 0..8 {
            assert!(!shell.rebuild_if_stale(&mut editor).unwrap());
        }
        assert_eq!(shell.take_stats().rebuilds, 0);
        assert_eq!(editor.host.store().revision(), revision);
        assert_eq!(editor.host.store().history().len(), history);
        assert_eq!(editor.panels_revision, panels);
    }
}

#[test]
fn pending_script_parent_keeps_its_handle_and_throttles_without_rebuilding() {
    let project = Project::new();
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    shell.offscreen = true;
    let (_input, job) = start(&mut editor, &mut shell);
    let revision = editor.host.store().revision();
    let history = editor.host.store().history().len();
    let panels = editor.panels_revision;
    for _ in 0..16 {
        let last = Instant::now();
        shell.script_job.as_mut().unwrap().1 = last;
        assert!(!shell.rebuild_if_stale(&mut editor).unwrap());
        let (pending, polled) = shell.script_job.as_ref().unwrap();
        assert_eq!(pending, &job);
        assert_eq!(*polled, last);
    }
    shell.script_job.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(1);
    let inventory = shell.monkey_background_work();
    assert_eq!(inventory["scriptJob"]["job"], job);
    assert_eq!(inventory["scriptJob"]["ageResolutionMs"], 1000);
    assert!(inventory["scriptJob"]["ageMs"].as_u64().unwrap() >= 1000);
    assert!(!shell.rebuild_if_stale(&mut editor).unwrap());
    assert_eq!(shell.script_job.as_ref().unwrap().0, job);
    assert_eq!(
        editor.action("jobs.status", json!({"job":job})).unwrap()["status"],
        "running"
    );
    assert_eq!(shell.take_stats().rebuilds, 0);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(editor.host.store().history().len(), history);
    assert_eq!(editor.panels_revision, panels);
}
