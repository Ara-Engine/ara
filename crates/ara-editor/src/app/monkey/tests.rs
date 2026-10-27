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
        json!({"kind":"pointerButton","point":[100.0,100.0],"button":3,"pressed":true}),
        json!({"kind":"keyState","scancode":26,"pressed":true}),
        json!({"kind":"focusLost"}),
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

#[test]
fn custom_input_callback_is_inside_the_durable_journal_and_panic_boundary() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let source = project.0.join("callbacks.jsonl");
    let lines = (0..3)
        .map(|step| {
            serde_json::to_string(&json!({
                "phase":"begin","seed":9,"step":step,"action":{"kind":"click","point":[step,2]}
            }))
            .unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source, lines).unwrap();
    let output = project.0.join("callback-report");
    let mut runner = Monkey::from_journal(&source, Duration::from_secs(30), &output).unwrap();
    let mut called = 0;
    while !runner.finished() {
        runner
            .step_with(&mut editor, &mut shell, |action, _, _, clipboard| {
                let journal = std::fs::read_to_string(output.join("monkey-events.jsonl")).unwrap();
                let last: Value = serde_json::from_str(journal.lines().last().unwrap()).unwrap();
                assert_eq!(last["phase"], "begin");
                assert_eq!(last["action"], serde_json::to_value(action).unwrap());
                called += 1;
                match called {
                    1 => {
                        clipboard.write("session-only UTF-8 🙂")?;
                        Ok(())
                    }
                    2 => {
                        assert_eq!(clipboard.read()?, "session-only UTF-8 🙂");
                        Err("controlled production input rejection".into())
                    }
                    _ => panic!("controlled production input panic"),
                }
            })
            .unwrap();
    }
    runner.finish().unwrap();
    assert_eq!(called, 3);
    assert!(!runner.succeeded());
    assert_eq!(runner.actions, 3);
    assert_eq!(runner.rejected, 1);
    assert_eq!(runner.panics.len(), 1);
    let events: Vec<Value> = std::fs::read_to_string(output.join("monkey-events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events.iter().filter(|row| row["phase"] == "begin").count(),
        3
    );
    assert_eq!(events.iter().filter(|row| row["phase"] == "end").count(), 3);
    assert!(events.iter().any(|row| {
        row["outcome"]
            .as_str()
            .is_some_and(|value| value.starts_with("panic:"))
    }));
}

fn main_surface() -> native::Surface {
    native::Surface {
        id: None,
        sdl: 7,
        points_to_logical: 2.,
        desktop: [100., 200., 450., 300.],
    }
}

#[test]
fn native_pointer_events_keep_motion_delta_virtual_desktop_and_cancel_release() {
    use ara_platform::Event;
    let project = Project::new();
    let (_, mut shell) = project.open();
    let mut adapter = native::Adapter::default();
    let main = main_surface();
    let pressed = adapter
        .events(&Action::Press { point: [20., 40.] }, &mut shell, &main, &[])
        .unwrap()
        .unwrap();
    assert_eq!(pressed.desktop_pointer, Some([110., 220.]));
    assert!(
        matches!(pressed.events.as_slice(),[(7,Event::MouseMotion{position,delta}),(7,Event::MouseButton{button:1,pressed:true})]
        if *position==[10.,20.]&&*delta==[0.,0.])
    );
    let moved = adapter
        .events(&Action::Move { point: [44., 64.] }, &mut shell, &main, &[])
        .unwrap()
        .unwrap();
    assert_eq!(moved.desktop_pointer, Some([122., 232.]));
    assert!(
        matches!(moved.events.as_slice(),[(7,Event::MouseMotion{position,delta})] if *position==[22.,32.]&&*delta==[12.,12.])
    );
    let released = adapter
        .events(
            &Action::Release {
                point: [44., 64.],
                cancel: true,
            },
            &mut shell,
            &main,
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        released.events.last(),
        Some((
            7,
            Event::MouseButton {
                button: 1,
                pressed: false
            }
        ))
    ));
    assert!(released.events.iter().any(|(_, event)| matches!(
        event,
        Event::Key {
            scancode: 41,
            pressed: true,
            ..
        }
    )));
    assert!(
        adapter
            .events(
                &Action::Move {
                    point: [f32::INFINITY, 0.]
                },
                &mut shell,
                &main,
                &[]
            )
            .is_err()
    );
}

#[test]
fn native_text_targets_a_real_mounted_field_and_preserves_utf8() {
    use ara_platform::Event;
    let project = Project::new();
    let (_, mut shell) = project.open();
    let id = shell
        .ui
        .nodes
        .iter()
        .enumerate()
        .find(|(index, node)| {
            shell.ui.shown(*index) && matches!(node.element.tag.as_str(), "field" | "input")
        })
        .map(|(_, node)| node.element.id.clone())
        .expect("real editor has a mounted search field");
    let mut adapter = native::Adapter::default();
    let input = adapter
        .events(
            &Action::Text {
                id: Some(id),
                value: "مرحبا\n🙂".into(),
            },
            &mut shell,
            &main_surface(),
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        input.events.first(),
        Some((7, Event::MouseMotion { .. }))
    ));
    assert!(
        input
            .events
            .iter()
            .any(|(_, event)| matches!(event,Event::TextInput{text} if text=="مرحبا\n🙂"))
    );
    let text = input
        .events
        .iter()
        .position(|(_, event)| matches!(event, Event::TextInput { .. }))
        .unwrap();
    let enter = input
        .events
        .iter()
        .position(|(_, event)| {
            matches!(
                event,
                Event::Key {
                    scancode: 40,
                    pressed: true,
                    ..
                }
            )
        })
        .unwrap();
    assert!(text < enter);
    let empty = adapter
        .events(
            &Action::Text {
                id: None,
                value: String::new(),
            },
            &mut shell,
            &main_surface(),
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(empty.events.iter().any(|(_, event)| matches!(
        event,
        Event::Key {
            scancode: 42,
            pressed: true,
            ..
        }
    )));
    assert!(
        adapter
            .events(
                &Action::Text {
                    id: Some("deleted-field".into()),
                    value: "1e308".into()
                },
                &mut shell,
                &main_surface(),
                &[]
            )
            .is_err()
    );
}

#[test]
fn floating_native_input_keeps_window_tags_until_a_main_pointer_action() {
    let project = Project::new();
    let (_, mut shell) = project.open();
    let main = main_surface();
    let floats = [native::Surface {
        id: Some("float-a".into()),
        sdl: 42,
        points_to_logical: 1.25,
        desktop: [600., 100., 320., 240.],
    }];
    let mut adapter = native::Adapter::default();
    let input = adapter
        .events(
            &Action::FloatingInput {
                surface: "float-a".into(),
                reverse: true,
            },
            &mut shell,
            &main,
            &floats,
        )
        .unwrap()
        .unwrap();
    assert!(input.events.iter().all(|(sdl, _)| *sdl == 42));
    assert_eq!(input.desktop_pointer, Some([760., 220.]));
    let key = adapter
        .events(&Action::Key { scancode: 41 }, &mut shell, &main, &floats)
        .unwrap()
        .unwrap();
    assert!(key.events.iter().all(|(sdl, _)| *sdl == 42));
    assert!(
        adapter
            .events(&Action::Tab { reverse: false }, &mut shell, &main, &[])
            .is_err()
    );
    let lost = adapter
        .events(&Action::FocusLost, &mut shell, &main, &[])
        .unwrap()
        .unwrap();
    assert!(matches!(
        lost.events.as_slice(),
        [(7, ara_platform::Event::FocusLost)]
    ));
    adapter
        .events(
            &Action::Click { point: [1., 1.] },
            &mut shell,
            &main,
            &floats,
        )
        .unwrap();
    let key = adapter
        .events(&Action::Key { scancode: 41 }, &mut shell, &main, &[])
        .unwrap()
        .unwrap();
    assert!(key.events.iter().all(|(sdl, _)| *sdl == 7));
}

#[test]
fn native_held_camera_input_does_not_synthesize_an_early_key_release() {
    use ara_platform::Event;
    let project = Project::new();
    let (_, mut shell) = project.open();
    let main = main_surface();
    let mut adapter = native::Adapter::default();
    let pressed = adapter
        .events(
            &Action::PointerButton {
                point: [100., 100.],
                button: 3,
                pressed: true,
            },
            &mut shell,
            &main,
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        pressed.events.last(),
        Some((
            7,
            Event::MouseButton {
                button: 3,
                pressed: true
            }
        ))
    ));
    let held = adapter
        .events(
            &Action::KeyState {
                scancode: 26,
                pressed: true,
            },
            &mut shell,
            &main,
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        held.events.as_slice(),
        [(
            7,
            Event::Key {
                scancode: 26,
                pressed: true,
                ..
            }
        )]
    ));
    let released = adapter
        .events(
            &Action::KeyState {
                scancode: 26,
                pressed: false,
            },
            &mut shell,
            &main,
            &[],
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        released.events.as_slice(),
        [(
            7,
            Event::Key {
                scancode: 26,
                pressed: false,
                ..
            }
        )]
    ));
}

#[test]
fn generated_budget_records_focus_loss_drain_separately_from_workload() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("drain");
    let mut runner = Monkey::new(1, Duration::from_secs(30), Some(1), &output).unwrap();
    runner.generator.enqueue([Action::KeyState {
        scancode: 26,
        pressed: true,
    }]);
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
    }
    runner.finish().unwrap();
    assert!(runner.succeeded());
    assert_eq!(runner.workload_actions, 1);
    assert_eq!(runner.cleanup_actions, 1);
    assert_eq!(runner.drain_actions, 1);
    let begins: Vec<Value> = std::fs::read_to_string(output.join("monkey-events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["phase"] == "begin")
        .collect();
    assert_eq!(begins.len(), 2);
    assert_eq!(begins[0]["action"]["kind"], "keyState");
    assert_eq!(begins[0]["cleanup"], false);
    assert_eq!(begins[1]["action"]["kind"], "focusLost");
    assert_eq!(begins[1]["cleanup"], true);
}

#[test]
fn capped_drain_fails_and_exact_replay_does_not_infer_extra_actions() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut runner = Monkey::new(
        1,
        Duration::from_secs(30),
        Some(1),
        &project.0.join("capped"),
    )
    .unwrap();
    runner.generator.enqueue([Action::KeyState {
        scancode: 26,
        pressed: true,
    }]);
    runner.step(&mut editor, &mut shell).unwrap();
    runner.draining_since = Some(Instant::now() - DRAIN);
    assert!(!runner.step(&mut editor, &mut shell).unwrap());
    assert!(runner.finished());
    assert!(!runner.succeeded());
    assert!(runner.cleanup_incomplete);

    let journal = project.0.join("held-only.jsonl");
    std::fs::write(
        &journal,
        serde_json::to_string(&json!({"phase":"begin","seed":1,"step":0,
        "action":{"kind":"keyState","scancode":26,"pressed":true}}))
        .unwrap(),
    )
    .unwrap();
    let mut replay = Monkey::from_journal(
        &journal,
        Duration::from_secs(30),
        &project.0.join("held-replay"),
    )
    .unwrap();
    replay.step(&mut editor, &mut shell).unwrap();
    assert!(replay.finished());
    assert_eq!(replay.actions, 1);
    assert_eq!(replay.cleanup_actions, 0);
    assert!(
        !replay.succeeded(),
        "a journal ending with a held gesture is incomplete"
    );
}
