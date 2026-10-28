//! Revision-guard proof with the real Host and a separate opt-in timing comparison.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("compile-projection-poll")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Compile poll","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_compilepoll","records":{}}),
            ),
        ] {
            let path = if path == "ara.project.json" {
                root.join(path)
            } else {
                root.join(ara_doc::AUTHORING_ROOT).join(path)
            };
            std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let parent = std::path::absolute(crate::test_target())
            .unwrap()
            .join("compile-projection-poll");
        assert!(self.0.starts_with(parent));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn settled(editor: &mut Editor, shell: &mut Shell) {
    // A read may itself deliver a startup notification. Preserve that new
    // revision as next-poll work, then establish a genuinely unchanged interval.
    for _ in 0..16 {
        shell.poll_compile_problems(editor).unwrap();
        if shell.compile_problems.polled_revision == Some(editor.panels_revision) {
            return;
        }
    }
    panic!("Initial compile-projection notifications did not settle");
}

#[test]
fn unchanged_revision_reads_once_and_a_real_diagnostic_revision_enables_the_next_read() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    settled(&mut editor, &mut shell);
    let revision = editor.panels_revision;
    let before = shell.compile_problems.reads;
    shell.compile_problems.polled_revision = None;
    for _ in 0..1000 {
        shell.poll_compile_problems(&mut editor).unwrap();
    }
    assert_eq!(editor.panels_revision, revision);
    assert_eq!(
        shell.compile_problems.reads,
        before + 1,
        "unchanged polls perform exactly one logs read"
    );
    assert_eq!(shell.compile_problems.polled_revision, Some(revision));
    let emitter = Emitter::default();
    editor.action("diag.report",json!({"engine":true,"diagnostics":[emitter.marker(0,"bound"),emitter.marker(1,"started"),emitter.row(1,"diagnostic",900,Severity::Error,"new compile diagnostic")]})).unwrap();
    editor.tick();
    for (connection, notification) in editor.host.drain_outbox() {
        if connection == ara_proto::ConnId::LOCAL {
            editor.notified(&notification);
        }
    }
    assert_ne!(
        editor.panels_revision, revision,
        "real bus notifications invalidate the projection"
    );
    let reads = shell.compile_problems.reads;
    let notified = editor.panels_revision;
    assert!(shell.poll_compile_problems(&mut editor).unwrap());
    assert_eq!(shell.compile_problems.reads, reads + 1);
    assert_eq!(
        shell.compile_problems.polled_revision,
        Some(notified),
        "capture the revision before reading, not after action/sync"
    );
    assert!(
        shell
            .compile_problems
            .entries()
            .any(|entry| entry.diagnostic.message == "new compile diagnostic")
    );
    settled(&mut editor, &mut shell);
    let reads = shell.compile_problems.reads;
    for _ in 0..1000 {
        assert!(!shell.poll_compile_problems(&mut editor).unwrap());
    }
    assert_eq!(shell.compile_problems.reads, reads);
}

#[test]
#[ignore = "opt-in idle compile-poll CPU measurement; writes only target/ux-p4"]
fn guarded_poll_cpu_benchmark_against_old_query_and_scalar_revision_check() {
    if std::env::var("ARA_EDITOR_BENCH_COMPILE_POLL").as_deref() != Ok("1") {
        return;
    }
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    settled(&mut editor, &mut shell);
    let iterations = 10_000u64;
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(editor.action("logs",json!({"group":false,"limit":PAGE,"since":shell.compile_problems.cursor,"severity":"info"})).unwrap());
    }
    let old_ns = started.elapsed().as_nanos();
    settled(&mut editor, &mut shell);
    let reads = shell.compile_problems.reads;
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(shell.poll_compile_problems(&mut editor).unwrap());
    }
    let guarded_ns = started.elapsed().as_nanos();
    assert_eq!(shell.compile_problems.reads, reads);
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(
            shell.compile_problems.polled_revision
                == Some(std::hint::black_box(editor.panels_revision)),
        );
    }
    let scalar_ns = started.elapsed().as_nanos();
    let report = json!({"iterations":iterations,"oldAlwaysQueryNs":old_ns,"guardedPollNs":guarded_ns,"scalarRevisionCheckNs":scalar_ns,
        "idleLogsReads":shell.compile_problems.reads-reads,"timing":"CPU wall-clock intervals; no GPU/visible window; no ratio assertion"});
    let output = crate::test_target().join("ux-p4/compile-poll-benchmark.json");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(output, ara_json::to_canonical_string(&report)).unwrap();
    println!("{report}");
}
