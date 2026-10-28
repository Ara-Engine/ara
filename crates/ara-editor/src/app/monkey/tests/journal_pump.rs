//! Real Editor/widget progress while a persisted-begin acknowledgement is pending.
use super::*;
use std::{
    io::{self, Write},
    sync::mpsc,
};

struct Slow {
    file: std::io::BufWriter<std::fs::File>,
    first: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    fail: bool,
    fail_after_first: bool,
    flushes_to_skip: u64,
}
impl Write for Slow {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.flushes_to_skip > 0 {
            self.flushes_to_skip -= 1;
        } else if let Some((entered, release)) = self.first.take() {
            let _ = entered.send(());
            release.recv().map_err(io::Error::other)?;
        }
        if self.fail {
            return Err(io::Error::other("injected begin flush failure"));
        }
        let result = self.file.flush();
        self.fail = self.fail_after_first;
        result
    }
}
struct Release(Option<mpsc::Sender<()>>);
impl Release {
    fn open(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}

#[test]
fn delayed_begin_keeps_widgets_and_host_live_then_dispatches_the_exact_action_once() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("async-begin");
    let mut runner = Monkey::new(17, Duration::from_secs(30), Some(1), &output).unwrap();
    let path = output.join("monkey-events.jsonl");
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    runner.journal = journal::Writer::test_sink(Slow {
        file: std::io::BufWriter::new(std::fs::File::create(&path).unwrap()),
        first: Some((entered, wait)),
        fail: false,
        fail_after_first: false,
        flushes_to_skip: 0,
    })
    .unwrap();
    let mut gate = Release(Some(release));
    runner.generator.enqueue([Action::Tab { reverse: true }]);
    let mut dispatched = 0;
    let mut callback = |action: &Action, _: &mut Editor, _: &mut Shell, _: &mut dyn Clipboard| {
        let records = std::fs::read_to_string(&path).unwrap();
        let last: Value = serde_json::from_str(records.lines().last().unwrap()).unwrap();
        assert_eq!(last["phase"], "begin");
        assert_eq!(last["action"], serde_json::to_value(action).unwrap());
        dispatched += 1;
        Ok(())
    };
    assert!(
        !runner
            .step_with(&mut editor, &mut shell, &mut callback)
            .unwrap()
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let staged = runner.pending_action.as_ref().unwrap().json.clone();
    // A staged action remains the same action even if its IO wait crosses the
    // workload budget. Teardown must not replace it with a newly generated one.
    runner.started = Instant::now() - Duration::from_secs(31);
    for _ in 0..3 {
        assert!(
            !runner
                .step_with(&mut editor, &mut shell, &mut callback)
                .unwrap()
        );
        assert_eq!(runner.pending_action.as_ref().unwrap().json, staged);
        editor.tick();
        shell.frame().unwrap();
    }
    // Drive an actual retained field, rather than substituting a fake model of input.
    let field = "search-hierarchy";
    assert!(shell.ui.shown(shell.ui.find(field).unwrap()));
    let mut clipboard = MemoryClipboard::default();
    shell.click(field, &mut clipboard).unwrap();
    shell
        .event(
            ara_ui::input::InputEvent::Edit(ara_ui_text::EditEvent::Insert(
                "still responsive".into(),
            )),
            &mut clipboard,
        )
        .unwrap();
    assert!(
        shell.ui.nodes[shell.ui.find(field).unwrap()]
            .text
            .as_ref()
            .is_some_and(|text| text.value.contains("still responsive"))
    );
    assert_eq!(runner.actions, 0);
    gate.open();
    let deadline = Instant::now() + Duration::from_secs(5);
    while runner.actions == 0 {
        runner
            .step_with(&mut editor, &mut shell, &mut callback)
            .unwrap();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(dispatched, 1);
    assert_eq!(runner.workload_actions, 1);
    assert!(runner.finished());
    runner.finish_with(&mut editor).unwrap();
    let records = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        records
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["phase"] == "begin")
            .count(),
        1
    );
}

#[test]
fn a_failed_begin_never_dispatches_and_the_final_report_fails_closed() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("failed-begin");
    let mut runner = Monkey::new(19, Duration::from_secs(30), Some(1), &output).unwrap();
    runner.journal = journal::Writer::test_sink(Slow {
        file: std::io::BufWriter::new(
            std::fs::File::create(output.join("monkey-events.jsonl")).unwrap(),
        ),
        first: None,
        fail: true,
        fail_after_first: false,
        flushes_to_skip: 0,
    })
    .unwrap();
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    let mut dispatched = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runner.finished() {
        runner
            .step_with(&mut editor, &mut shell, |_, _, _, _| {
                dispatched += 1;
                Ok(())
            })
            .unwrap();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(dispatched, 0);
    assert_eq!(runner.actions, 0);
    assert!(runner.finish_with(&mut editor).is_err());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["accepted"], false);
    assert_eq!(report["termination"], "journal-error");
    assert!(
        report["journalRecorder"]["error"]
            .as_str()
            .unwrap()
            .contains("injected begin flush failure")
    );
    assert_eq!(
        report["journalRecorder"]["abortedAction"]["wasDispatched"],
        false
    );
}

#[test]
fn acknowledgement_deadline_fails_evidence_without_dispatching_the_staged_action() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("begin-deadline");
    let mut runner = Monkey::new(23, Duration::from_secs(30), Some(1), &output).unwrap();
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    runner.journal = journal::Writer::test_sink(Slow {
        file: std::io::BufWriter::new(
            std::fs::File::create(output.join("monkey-events.jsonl")).unwrap(),
        ),
        first: Some((entered, wait)),
        fail: false,
        fail_after_first: false,
        flushes_to_skip: 0,
    })
    .unwrap();
    let mut gate = Release(Some(release));
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    let mut dispatched = 0;
    runner
        .step_with(&mut editor, &mut shell, |_, _, _, _| {
            dispatched += 1;
            Ok(())
        })
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runner.pending_action.as_mut().unwrap().since =
        Instant::now() - JOURNAL_ACK - Duration::from_secs(1);
    assert!(
        !runner
            .step_with(&mut editor, &mut shell, |_, _, _, _| {
                dispatched += 1;
                Ok(())
            })
            .unwrap()
    );
    assert_eq!(dispatched, 0);
    assert!(runner.finished());
    assert!(
        runner
            .journal_error
            .as_ref()
            .unwrap()
            .contains("acknowledgement exceeded")
    );
    gate.open();
    assert!(runner.finish_with(&mut editor).is_err());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["actions"], 0);
    assert_eq!(report["accepted"], false);
    assert_eq!(
        report["journalRecorder"]["abortedAction"]["wasDispatched"],
        false
    );
}

#[test]
fn a_terminal_record_flush_error_rejects_the_authoritative_final_report() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("terminal-flush");
    let mut runner = Monkey::new(29, Duration::from_secs(30), Some(100), &output).unwrap();
    runner.journal = journal::Writer::test_sink(Slow {
        file: std::io::BufWriter::new(
            std::fs::File::create(output.join("monkey-events.jsonl")).unwrap(),
        ),
        first: None,
        fail: false,
        fail_after_first: true,
        flushes_to_skip: 0,
    })
    .unwrap();
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    runner.step(&mut editor, &mut shell).unwrap();
    assert_eq!(
        runner.actions, 1,
        "Begin flush succeeded and the action really ran"
    );
    assert!(runner.finish_with(&mut editor).is_err());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["accepted"], false);
    assert_eq!(report["journalRecorder"]["finalDrained"], false);
    assert!(report["journalRecorder"]["error"].is_string());
    assert_eq!(report["journalRecorder"]["recordsNotQueued"], 0);
}

#[test]
fn cleanup_deadline_aborts_only_the_undispatched_action_and_replay_excludes_it() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("cleanup-deadline");
    let mut runner = Monkey::new(31, Duration::from_secs(30), Some(1), &output).unwrap();
    let path = output.join("monkey-events.jsonl");
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    runner.journal = journal::Writer::test_sink(Slow {
        file: std::io::BufWriter::new(std::fs::File::create(&path).unwrap()),
        first: Some((entered, wait)),
        fail: false,
        fail_after_first: false,
        flushes_to_skip: 1,
    })
    .unwrap();
    let mut gate = Release(Some(release));
    runner.generator.enqueue([Action::KeyState {
        scancode: 26,
        pressed: true,
    }]);
    runner.step(&mut editor, &mut shell).unwrap();
    assert_eq!(runner.actions, 1);
    let mut callbacks = 0;
    runner
        .step_with(&mut editor, &mut shell, |_, _, _, _| {
            callbacks += 1;
            Ok(())
        })
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(runner.pending_action.as_ref().unwrap().recorded.cleanup);
    runner.draining_since = Some(Instant::now() - DRAIN);
    assert!(
        !runner
            .step_with(&mut editor, &mut shell, |_, _, _, _| {
                callbacks += 1;
                Ok(())
            })
            .unwrap()
    );
    assert_eq!(callbacks, 0);
    assert!(runner.cleanup_incomplete);
    assert!(runner.journal_error.is_none());
    gate.open();
    runner.finish_with(&mut editor).unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["accepted"], false);
    assert_eq!(report["termination"], "cleanup-timeout");
    assert_eq!(report["actions"], 1);
    assert_eq!(report["cleanupActions"], 0);
    assert_eq!(report["endDrain"]["actions"], 0);
    assert_eq!(report["journalRecorder"]["finalDrained"], true);
    assert_eq!(report["journalRecorder"]["error"], Value::Null);
    let replay = journal::read(&path).unwrap();
    assert_eq!(replay.actions.len(), 1);
    assert!(matches!(
        &replay.actions[0].action,
        Action::KeyState {
            scancode: 26,
            pressed: true
        }
    ));
}
