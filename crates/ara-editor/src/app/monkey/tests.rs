//! Determinism, durable reporting and malformed inputs use the actual editor authority.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("monkey-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Seeded editor","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_monkey01","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor = Editor::open(&self.0).unwrap();
        let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
        shell.offscreen = true;
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn short_real_editor_run_writes_replay_and_preserves_window_isolation() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("evidence");
    let mut monkey = Monkey::new(42, Duration::from_secs(60), Some(8), &output).unwrap();
    while !monkey.finished() {
        monkey.step(&mut editor, &mut shell).unwrap();
        shell.frame().unwrap();
        monkey.record_frame(Duration::from_millis(2));
        assert!(shell.window_request.is_none());
    }
    monkey.finish().unwrap();
    assert!(monkey.succeeded());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["seed"], 42);
    assert_eq!(report["actions"], 8);
    assert_eq!(report["panics"], json!([]));
    assert_eq!(report["frames"], 8);
    let journal = std::fs::read_to_string(output.join("monkey-events.jsonl")).unwrap();
    let events: Vec<Value> = journal
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["phase"] == "begin")
            .count(),
        8
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["phase"] == "end")
            .count(),
        8
    );
}

#[test]
fn editor_action_rejects_scalar_and_array_params_without_unwinding() {
    let project = Project::new();
    let (mut editor, _) = project.open();
    for value in [
        json!([]),
        json!([1]),
        json!(true),
        json!(1e308),
        json!("NaN"),
    ] {
        let result = catch_unwind(AssertUnwindSafe(|| editor.action("editor.camera", value)));
        let error = result
            .expect("malformed action params must not panic")
            .expect_err("malformed params must be rejected");
        assert_eq!(error.0.code, -32602);
    }
}

#[test]
fn bounded_options_force_hidden_windows_and_default_to_a_long_soak() {
    let mut options = super::super::setup::Options::default();
    assert!(!options.offscreen());
    options.verify_monkey = true;
    assert!(options.offscreen());
    assert_eq!(options.monkey_seconds, 1800);
    assert_eq!(options.monkey_steps, None);
}

#[test]
fn exact_replay_preserves_recorded_actions_and_multilingual_text() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let source = project.0.join("recorded.jsonl");
    let actions = [
        json!({"kind":"protocol","method":"editor.focus","params":{"panel":"hierarchy"},"raw":false}),
        json!({"kind":"text","id":null,"value":"مرحبا\n🙂"}),
        json!({"kind":"protocol","method":"editor.camera","params":["NaN"],"raw":false}),
    ];
    let lines = actions
        .iter()
        .enumerate()
        .map(|(step, action)| {
            serde_json::to_string(&json!({"phase":"begin","seed":77,"step":step,"action":action}))
                .unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source, lines).unwrap();
    let output = project.0.join("replayed");
    let mut runner = Monkey::from_journal(&source, Duration::from_secs(30), &output).unwrap();
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
        shell.frame().unwrap();
    }
    runner.finish().unwrap();
    assert!(runner.succeeded());
    let actual: Vec<Value> = std::fs::read_to_string(output.join("monkey-events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["phase"] == "begin")
        .map(|record| record["action"].clone())
        .collect();
    assert_eq!(actual, actions);
    assert!(source.is_file());
}
