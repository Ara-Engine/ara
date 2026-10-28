//! Determinism, durable reporting and malformed inputs use the actual editor authority.
mod journal_pump;
use super::*;
use crate::shell::commands;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn offscreen_output_evidence_counts_gpu_submissions_separately_from_native_presentation() {
    let project = Project::new();
    let output = project.0.join("output-counters");
    let mut runner = Monkey::new(42, Duration::from_secs(60), Some(1), &output).unwrap();
    let sample = super::super::responsiveness::Sample {
        offscreen_output: true,
        gpu_surface_submissions: 3,
        native_presentations: 0,
        presented: true,
        ..Default::default()
    };
    runner.record_frame_with_stages(Duration::from_millis(2), &sample);
    runner.finish().unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["offscreenGpuOutput"], true);
    assert_eq!(report["gpuSurfaceSubmissions"], 3);
    assert_eq!(report["nativePresentations"], 0);
    assert_eq!(
        report["accepted"], false,
        "Output counters alone never establish acceptance"
    );
}

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target().join("monkey-tests").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(root.join(".ara-monkey-fixture"), b"fresh test scratch\n").unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Seeded editor","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_monkey01","records":{}}),
            ),
        ] {
            let physical = if path == "ara.project.json" {
                root.join(path)
            } else {
                root.join(ara_doc::AUTHORING_ROOT).join(path)
            };
            std::fs::write(physical, ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
        shell.offscreen = true;
        (editor, shell)
    }
    fn open_seeded(&self, seed: u64) -> (Editor, Shell) {
        let mut editor = Editor::open_config(
            &self.0,
            Some(self.0.join(".ara/test-user.json")),
            ara_host::HostConfig {
                seed: Some(seed),
                ..Default::default()
            },
        )
        .unwrap();
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
    monkey.finish_with(&mut editor).unwrap();
    assert!(monkey.succeeded());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["seed"], 42);
    assert_eq!(report["actions"], 8);
    assert_eq!(report["panics"], json!([]));
    assert_eq!(report["frames"], 8);
    assert_eq!(report["accepted"], true);
    assert_eq!(report["soakRequested"], false);
    assert_eq!(report["soakAccepted"], false);
    assert_eq!(report["coverageComplete"], false);
    assert_eq!(report["termination"], "workload-step-limit");
    assert_eq!(report["finalDiagnosticsCollected"], true);
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
fn an_unmarked_project_is_rejected_before_the_first_action() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    std::fs::remove_file(project.0.join(".ara-monkey-fixture")).unwrap();
    let before = editor.host.store().revision();
    let output = project.0.join("unmarked");
    let mut runner = Monkey::new(1, Duration::from_secs(30), Some(1), &output).unwrap();
    assert!(runner.step(&mut editor, &mut shell).is_err());
    assert_eq!(runner.actions, 0);
    assert_eq!(editor.host.store().revision(), before);
    assert_eq!(
        std::fs::read(output.join("monkey-events.jsonl")).unwrap(),
        b""
    );
    std::fs::write(
        project.0.join(".ara-behaviour-fixture"),
        b"fresh test scratch\n",
    )
    .unwrap();
    assert!(runner.step(&mut editor, &mut shell).unwrap());
    runner.finish_with(&mut editor).unwrap();
    assert!(runner.succeeded());
}

#[test]
fn shutdown_collects_diagnostics_emitted_after_the_final_action() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("final-tail");
    let mut runner = Monkey::new(1, Duration::from_secs(30), Some(1), &output).unwrap();
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    runner.step(&mut editor, &mut shell).unwrap();
    assert!(runner.finished());
    assert!(
        !runner.succeeded(),
        "the final diagnostic tail is still pending"
    );
    editor.report_error("controlled error after the last monkey action");
    runner.finish_with(&mut editor).unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(
        report["errorDiagnosticsDuring"][crate::codes::ACTION.to_string()],
        1
    );
    assert_eq!(report["finalDiagnosticsCollected"], true);
    let journal = std::fs::read_to_string(output.join("monkey-events.jsonl")).unwrap();
    assert!(
        journal
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .any(|row| {
                row["phase"] == "diagnostic"
                    && row["entry"]["diagnostic"]["message"]
                        == "controlled error after the last monkey action"
            })
    );
}

#[test]
fn diagnostic_bus_loss_prevents_an_exact_count_acceptance_claim() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut runner =
        Monkey::new(1, Duration::from_secs(30), Some(1), &project.0.join("lost")).unwrap();
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    runner.step(&mut editor, &mut shell).unwrap();
    let dropped =
        ara_diag::Diagnostic::warning(ara_diag::codes::DIAG_DROPPED, "controlled bus loss report");
    editor
        .action(
            "diag.report",
            json!({"engine":true,"diagnostics":[dropped]}),
        )
        .unwrap();
    runner.finish_with(&mut editor).unwrap();
    assert!(runner.diagnostic_bus_loss);
    assert!(!runner.succeeded());
}

#[test]
fn an_elapsed_duration_without_coverage_or_native_input_is_not_a_soak_pass() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let revision = editor.host.store().revision();
    let output = project.0.join("incomplete-soak");
    let mut runner = Monkey::new(1, Duration::from_secs(1800), None, &output).unwrap();
    runner.generator.enqueue([Action::Tab { reverse: false }]);
    runner.step(&mut editor, &mut shell).unwrap();
    assert!(runner.is_soak());
    assert!(!runner.finished());
    // Exercise the acceptance predicate without waiting thirty minutes in a unit test.
    runner.started = Instant::now() - Duration::from_secs(1800);
    // Use the real nonblocking resource/file drain, not a synthetic elapsed-time
    // assertion that bypasses the final authority inventory.
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
        editor.tick();
    }
    runner.finish_with(&mut editor).unwrap();
    assert!(runner.finished());
    assert!(
        runner.succeeded(),
        "bounded crash/resource checks are independent of soak coverage"
    );
    assert!(!runner.soak_succeeded());
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["accepted"], false);
    assert_eq!(report["soakAccepted"], false);
    assert_eq!(report["soakRequested"], true);
    assert_eq!(report["durationCompleted"], true);
    assert_eq!(report["cleanupIncomplete"], false);
    assert_eq!(report["registryCommands"], json!({}));
    assert_eq!(report["shellOrdinarySweepCompleted"], false);
    assert!(report["shellDrainDeferredProbes"].as_u64().unwrap() > 0);
    assert!(
        !report["missingCommandEdgeCases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn native_inputs_interleave_the_complete_command_edge_sweep() {
    let project = Project::new();
    let (editor, shell) = project.open();
    let mut generator = Generator::new(42);
    generator.initialize(&editor);
    let mut cases_seen: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut native_seen = BTreeSet::new();
    let required = commands::all().len() * actions::edge_case_count();
    let mut reached_protocol = false;
    let mut typed = typed::State::default();
    let mut catalog = catalog::State::default();
    for _ in 0..required * 2 + 128 {
        let action = generator.next(&editor, &shell, &mut typed, &mut catalog);
        if let Action::Command { id, target } = &action {
            cases_seen
                .entry(id.clone())
                .or_default()
                .insert(actions::edge_case(target).unwrap());
        } else if matches!(action, Action::Protocol { .. }) {
            reached_protocol = true;
            break;
        } else {
            native_seen.insert(
                serde_json::to_value(action).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
    }
    assert!(
        reached_protocol,
        "command sweep must eventually reach protocol probes"
    );
    for spec in commands::all() {
        assert_eq!(
            cases_seen[spec.id].len(),
            actions::edge_case_count(),
            "missing cases for {}",
            spec.id
        );
    }
    for kind in [
        "tab",
        "wheel",
        "text",
        "press",
        "release",
        "pointerButton",
        "keyState",
        "focusLost",
    ] {
        assert!(
            native_seen.contains(kind),
            "missing early native input {kind}"
        );
    }
}

#[test]
fn loaded_dynamic_cases_use_real_shell_availability_and_final_authority_reconciliation() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let revision = editor.host.store().revision();
    let camera = editor.state.camera;
    let mut catalog = catalog::State::default();
    catalog.observe(&editor.extensions);
    // Consume generated native cases without recording attempts: this test only
    // offers late C# cases, so it must never claim complete native coverage.
    while catalog.next().is_some() {}
    editor.extensions = crate::extensions::Catalog {
        commands: ["tools.fixture_zeta", "tools.fixture_alpha"]
            .into_iter()
            .map(|id| crate::extensions::Command {
                id: id.into(),
                title: id.into(),
                summary: "Controlled loaded registration".into(),
                enabled: false,
                disabled_reason: Some("Fixture callback is disabled".into()),
            })
            .collect(),
        generation: Some("fixture-generation".into()),
        ..Default::default()
    };
    let mut generator = Generator::new(73);
    generator.initialize(&editor);
    let mut typed = typed::State::default();
    let mut clipboard = MemoryClipboard::default();
    let mut offered = BTreeMap::<String, BTreeSet<usize>>::new();
    for _ in 0..actions::edge_case_count() * 2 {
        let action = generator.next(&editor, &shell, &mut typed, &mut catalog);
        let Action::Command { id, target } = &action else {
            panic!("Expected a loaded C# registration probe")
        };
        let error = action
            .run(&mut editor, &mut shell, &mut clipboard)
            .unwrap_err();
        assert!(error.to_string().contains("Fixture callback is disabled"));
        let case = actions::edge_case(target).unwrap();
        catalog.record(id, case);
        offered.entry(id.clone()).or_default().insert(case);
    }
    assert_eq!(offered.len(), 2);
    assert!(
        offered
            .values()
            .all(|cases| cases.len() == actions::edge_case_count())
    );
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(editor.state.camera, camera);
    // A window-side snapshot is insufficient: the ordinary authority query must
    // agree after asynchronous registration/controls have actually published.
    let authority = editor.action("editor.commands", json!({})).unwrap();
    catalog.reconcile_final(&authority, true);
    assert!(!catalog.final_matched());
    assert!(!catalog.complete());
}

#[test]
fn seeded_host_and_streamed_journal_replay_mint_the_same_actual_entity_ids() {
    let seed = 104729;
    let project = Project::new();
    let (mut editor, mut shell) = project.open_seeded(seed);
    let output = project.0.join("seeded-host");
    let mut runner = Monkey::new(seed, Duration::from_secs(30), Some(1), &output).unwrap();
    assert!(runner.record_host_seed(seed + 1).is_err());
    runner.record_host_seed(seed).unwrap();
    runner.generator.enqueue([Action::TypedCommand {
        name: "entity.create".into(),
        doc: "sc_monkey01".into(),
        args: json!({"name":"Minted by seeded Host"}),
        case: "registeredExample".into(),
        changed_fields: 0,
    }]);
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
        editor.tick();
    }
    runner.finish_with(&mut editor).unwrap();
    let created: Vec<_> = editor
        .host
        .store()
        .workspace()
        .doc("sc_monkey01")
        .unwrap()
        .records()
        .keys()
        .cloned()
        .collect();
    assert_eq!(created.len(), 1);
    let journal = output.join("monkey-events.jsonl");
    assert_eq!(Monkey::journal_seed(&journal).unwrap(), seed);

    let replay_project = Project::new();
    let (mut replay_editor, mut replay_shell) =
        replay_project.open_seeded(Monkey::journal_seed(&journal).unwrap());
    assert_eq!(editor.session, replay_editor.session);
    let replay_output = replay_project.0.join("seeded-replay");
    let mut replay =
        Monkey::from_journal(&journal, Duration::from_secs(30), &replay_output).unwrap();
    replay.record_host_seed(seed).unwrap();
    while !replay.finished() {
        replay.step(&mut replay_editor, &mut replay_shell).unwrap();
        replay_editor.tick();
    }
    replay.finish_with(&mut replay_editor).unwrap();
    let replayed: Vec<_> = replay_editor
        .host
        .store()
        .workspace()
        .doc("sc_monkey01")
        .unwrap()
        .records()
        .keys()
        .cloned()
        .collect();
    assert_eq!(created, replayed);
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(replay_output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["hostSeed"], seed);
    assert_eq!(report["hostSeedProvenance"], "startup");
    assert_eq!(report["replayOriginalHostSeed"], seed);
    assert_eq!(report["hostSeedMatchesReplay"], true);
    assert!(replay.record_host_seed(seed).is_err());
}

#[test]
fn typed_catalog_and_seven_case_sweep_follow_real_host_descriptors_in_fixed_order() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let revision = editor.host.store().revision();
    let mut typed = typed::State::default();
    let mut discovery = Vec::new();
    while let Some(action) = typed.discovery_action() {
        discovery.push(serde_json::to_value(&action).unwrap());
        typed
            .run(&action, &mut editor, &mut shell)
            .unwrap()
            .unwrap();
    }
    assert_eq!(discovery.len(), 3);
    assert_eq!(discovery[0]["method"], "schema.list");
    assert_eq!(discovery[0]["params"]["kind"], "command");
    assert_eq!(discovery[2]["method"], "jobs.list");
    let listed = editor
        .action("schema.list", json!({"kind":"command"}))
        .unwrap();
    let expected: BTreeSet<_> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    let report = typed.report();
    assert_eq!(report["typedCommandCatalogErrors"], json!([]));
    assert_eq!(report["typedCommandCatalog"], json!(expected));
    assert_eq!(
        report["typedCommandCoverageComplete"], false,
        "Generation is not execution"
    );
    let mut actual = BTreeMap::<String, Vec<String>>::new();
    let mut ordered = Vec::new();
    let mut references = false;
    let mut numeric = false;
    let mut nan_text = false;
    while let Some(action) = typed.next(&editor) {
        let Action::TypedCommand {
            name,
            doc,
            args,
            case,
            changed_fields,
        } = action
        else {
            panic!("Catalog generation must not produce unrecorded side effects");
        };
        assert!(editor.host.store().workspace().doc(&doc).is_some());
        if name == "transform.set" {
            references |= case == "missingOrDeletedReferences"
                && args.to_string().contains("e_deleted001")
                && changed_fields > 0;
            numeric |= case == "hugePositiveNumbers"
                && args["position"][0].as_f64() == Some(1e308)
                && changed_fields > 0;
            nan_text |=
                case == "nanAsText" && args.to_string().contains("\"NaN\"") && changed_fields > 0;
        }
        ordered.push((name.clone(), case.clone()));
        actual.entry(name).or_default().push(case);
    }
    assert_eq!(actual.keys().cloned().collect::<BTreeSet<_>>(), expected);
    assert!(actual.values().all(|cases| cases.len() == 7));
    assert!(ordered.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(
        references && numeric && nan_text,
        "Schema-directed values must reach actual transform args"
    );
    assert_eq!(editor.host.store().revision(), revision);
    assert!(
        !typed.complete(),
        "Generated probes must not count as offered cmd.apply calls"
    );
}

#[test]
fn typed_calls_report_actual_commits_and_replay_exact_doc_args_and_case() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let output = project.0.join("typed-recording");
    let mut runner = Monkey::new(97, Duration::from_secs(30), Some(5), &output).unwrap();
    for _ in 0..3 {
        runner.step(&mut editor, &mut shell).unwrap();
    }
    runner.generator.enqueue([
        Action::TypedCommand {
            name: "entity.create".into(),
            doc: "sc_monkey01".into(),
            args: json!({"id":"e_typedprobe","name":"Typed probe","components":{"Transform":{}}}),
            case: "registeredExample".into(),
            changed_fields: 0,
        },
        Action::TypedCommand {
            name: "transform.set".into(),
            doc: "sc_monkey01".into(),
            args: json!({"entities":["e_typedprobe"],"position":["NaN",0,0]}),
            case: "nanAsText".into(),
            changed_fields: 1,
        },
    ]);
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
    }
    runner.finish_with(&mut editor).unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["typedCommandCounts"]["entity.create"]["attempts"], 1);
    assert_eq!(report["typedCommandCounts"]["entity.create"]["accepted"], 1);
    assert_eq!(
        report["typedCommandCounts"]["entity.create"]["committedTransactions"],
        1
    );
    assert_eq!(report["typedCommandCounts"]["transform.set"]["attempts"], 1);
    assert_eq!(report["typedCommandCounts"]["transform.set"]["accepted"], 0);
    assert_eq!(
        report["typedCommandCounts"]["transform.set"]["committedTransactions"],
        0
    );
    assert_eq!(report["typedCommandCoverageComplete"], false);
    assert_eq!(report["hostJobResources"]["drained"], true);
    assert_eq!(report["hostJobResources"]["hostUnsaved"], false);
    assert_eq!(report["hostJobResources"]["fileFlushPending"], false);
    let replay_project = Project::new();
    let (mut replay_editor, mut replay_shell) = replay_project.open();
    let replay_output = replay_project.0.join("typed-replay");
    let journal = output.join("monkey-events.jsonl");
    let mut replay =
        Monkey::from_journal(&journal, Duration::from_secs(30), &replay_output).unwrap();
    while !replay.finished() {
        replay.step(&mut replay_editor, &mut replay_shell).unwrap();
    }
    replay.finish_with(&mut replay_editor).unwrap();
    let actions = |path: &Path| {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|row| row["phase"] == "begin")
            .map(|row| row["action"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        actions(&journal),
        actions(&replay_output.join("monkey-events.jsonl"))
    );
    assert_eq!(
        replay.typed.report()["typedCommandCounts"]["entity.create"]["committedTransactions"],
        1
    );
    assert!(
        replay_editor
            .host
            .store()
            .workspace()
            .doc("sc_monkey01")
            .unwrap()
            .record("e_typedprobe")
            .is_some()
    );
}

#[test]
fn production_stage_measurements_are_retained_only_for_spikes_and_count_each_frame_once() {
    let project = Project::new();
    let mut runner = Monkey::new(
        7,
        Duration::from_secs(30),
        Some(1),
        &project.0.join("stages"),
    )
    .unwrap();
    let stages = super::super::responsiveness::Sample {
        tick_ms: 7.,
        geometry_ms: 12.,
        host_wait_ms: Some(80.),
        ..Default::default()
    };
    runner.measurements.record(
        measurements::Stage::ReportSubmit,
        Duration::from_millis(110),
    );
    runner.record_frame_with_stages(Duration::from_millis(4), &stages);
    assert!(runner.slowest.is_empty());
    runner.record_frame_with_stages(Duration::from_millis(125), &stages);
    runner.record_frame(Duration::from_millis(3));
    assert_eq!(runner.frames, 3);
    assert_eq!(runner.spikes, 1);
    assert_eq!(runner.slowest.len(), 1);
    assert_eq!(runner.slowest[0]["stages"]["tick_ms"], 7.);
    assert_eq!(runner.slowest[0]["stages"]["host_wait_ms"], 80.);
    assert_eq!(runner.slowest[0]["harnessStages"]["reportSubmitMs"], 110.);
    runner.finish().unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(runner.output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(
        report["harnessTimings"]["stages"]["reportSubmitMs"]["calls"],
        1
    );
    assert_eq!(
        report["harnessTimings"]["stages"]["reportSubmitMs"]["totalMs"],
        110.
    );
}

#[test]
fn a_rejected_catalog_discovery_cannot_report_complete_typed_coverage() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let revision = editor.host.store().revision();
    let mut typed = typed::State::default();
    let invalid = Action::Protocol {
        method: "schema.list".into(),
        params: json!({"kind":"unknown-kind"}),
        raw: false,
    };
    assert!(
        typed
            .run(&invalid, &mut editor, &mut shell)
            .unwrap()
            .is_err()
    );
    while let Some(action) = typed.discovery_action() {
        typed
            .run(&action, &mut editor, &mut shell)
            .unwrap()
            .unwrap();
    }
    let report = typed.report();
    assert_eq!(report["typedCommandCatalogReady"], true);
    assert!(
        !report["typedCommandCatalogErrors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(report["typedCommandCoverageComplete"], false);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn final_catalog_drift_is_detected_against_the_real_startup_authority_snapshot() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut typed = typed::State::default();
    while let Some(action) = typed.discovery_action() {
        typed
            .run(&action, &mut editor, &mut shell)
            .unwrap()
            .unwrap();
    }
    let mut final_catalog = editor
        .action("schema.list", json!({"kind":"command"}))
        .unwrap();
    typed.validate_final_catalog(&final_catalog).unwrap();
    assert!(typed.final_catalog_matched());
    // The authority may gain external C# commands after loading a module. Its
    // descriptor must not silently fall outside a startup-only coverage claim.
    final_catalog["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"fixture.new.external.command","kind":"command"}));
    typed.validate_final_catalog(&final_catalog).unwrap();
    assert!(!typed.final_catalog_matched());
    let report = typed.report();
    assert!(
        !report["typedCommandCatalogErrors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(report["typedCommandCoverageComplete"], false);
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
    assert_eq!(Monkey::journal_seed(&source).unwrap(), 77);
    let output = project.0.join("replayed");
    let mut runner = Monkey::from_journal(&source, Duration::from_secs(30), &output).unwrap();
    while !runner.finished() {
        runner.step(&mut editor, &mut shell).unwrap();
        shell.frame().unwrap();
    }
    runner.finish_with(&mut editor).unwrap();
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
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("monkey.json")).unwrap())
            .unwrap();
    assert_eq!(report["replayOriginalHostSeed"], Value::Null);
    assert_eq!(report["replayOriginalHostSeedProvenance"], "unknown");
    assert_eq!(report["hostSeedMatchesReplay"], Value::Null);
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
    let deadline = Instant::now() + Duration::from_secs(5);
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
        assert!(
            Instant::now() < deadline,
            "journal callback test exceeded its bounded pump"
        );
        editor.tick();
        std::thread::yield_now();
    }
    runner.finish_with(&mut editor).unwrap();
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
    runner.finish_with(&mut editor).unwrap();
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
