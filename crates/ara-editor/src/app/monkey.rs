//! Seeded editor soak: real retained input, registry commands and protocol validation.
//!
//! Every action is flushed before it runs. If the process stops responding, the last
//! `begin` without an `end` identifies the exact action, seed and sequence to replay.
mod actions;
pub(super) mod native;
mod play;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod watchdog;

use crate::{
    Editor,
    shell::{Shell, commands},
};
pub(super) use actions::Action;
use actions::{Generator, MemoryClipboard};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::{BufWriter, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SPIKE: Duration = Duration::from_millis(100);
const HANG: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(30);

struct Recorded {
    action: Action,
    cleanup: bool,
}

/// One production-loop runner with durable actions and an injectable input router.
pub(super) struct Monkey {
    seed: u64,
    budget: Duration,
    max_steps: Option<u64>,
    started: Instant,
    output: PathBuf,
    journal: BufWriter<File>,
    generator: Generator,
    replay: Option<VecDeque<Recorded>>,
    clipboard: MemoryClipboard,
    play: play::State,
    actions: u64,
    workload_actions: u64,
    cleanup_actions: u64,
    drain_actions: u64,
    draining_since: Option<Instant>,
    cleanup_incomplete: bool,
    held_buttons: BTreeSet<u8>,
    held_keys: BTreeSet<i32>,
    frames: u64,
    rejected: u64,
    panics: Vec<Value>,
    hangs: u64,
    spikes: u64,
    max_frame_ms: f64,
    slowest: Vec<Value>,
    commands: BTreeMap<String, u64>,
    command_cases: BTreeMap<String, BTreeSet<usize>>,
    command_successes: BTreeMap<String, u64>,
    methods: BTreeMap<String, u64>,
    ui_actions: BTreeMap<String, u64>,
    diagnostics: BTreeMap<String, u64>,
    baseline_diagnostics: BTreeMap<String, u64>,
    diagnostic_counts: BTreeMap<u64, u64>,
    diagnostics_truncated: u64,
    final_diagnostics: bool,
    diagnostic_bus_loss: bool,
    native_events: u64,
    log_cursor: u64,
    stopped: bool,
    last_action: Option<Value>,
    boundary_failures: usize,
    watchdog: watchdog::Watchdog,
}

impl Monkey {
    /// Start a durable replay journal; the supplied output belongs to a scratch run.
    pub fn new(
        seed: u64,
        budget: Duration,
        max_steps: Option<u64>,
        output: &Path,
    ) -> ara_ui::Result<Self> {
        if budget.is_zero() || max_steps == Some(0) {
            return Err("monkey duration and optional action limit must be positive".into());
        }
        std::fs::create_dir_all(output)?;
        Ok(Self {
            seed,
            budget,
            max_steps,
            started: Instant::now(),
            output: output.into(),
            journal: BufWriter::new(File::create(output.join("monkey-events.jsonl"))?),
            generator: Generator::new(seed),
            replay: None,
            clipboard: MemoryClipboard::default(),
            play: play::State::default(),
            actions: 0,
            workload_actions: 0,
            cleanup_actions: 0,
            drain_actions: 0,
            draining_since: None,
            cleanup_incomplete: false,
            held_buttons: BTreeSet::new(),
            held_keys: BTreeSet::new(),
            frames: 0,
            rejected: 0,
            panics: Vec::new(),
            hangs: 0,
            spikes: 0,
            max_frame_ms: 0.,
            slowest: Vec::new(),
            commands: BTreeMap::new(),
            command_cases: BTreeMap::new(),
            command_successes: BTreeMap::new(),
            methods: BTreeMap::new(),
            ui_actions: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            baseline_diagnostics: BTreeMap::new(),
            diagnostic_counts: BTreeMap::new(),
            diagnostics_truncated: 0,
            final_diagnostics: false,
            diagnostic_bus_loss: false,
            native_events: 0,
            log_cursor: 0,
            stopped: false,
            last_action: None,
            boundary_failures: 0,
            watchdog: watchdog::Watchdog::new(output.into(), HANG)?,
        })
    }

    /// Replay the recorded actions verbatim, independent of later UI ordering or RNG choices.
    pub fn from_journal(path: &Path, budget: Duration, output: &Path) -> ara_ui::Result<Self> {
        let input = std::fs::canonicalize(path)?;
        if std::fs::canonicalize(output)
            .ok()
            .is_some_and(|output| input == output.join("monkey-events.jsonl"))
        {
            return Err(
                "Replay requires a new output directory so the original journal stays intact"
                    .into(),
            );
        }
        let mut actions = VecDeque::new();
        let mut seed = 0;
        for (line, text) in std::fs::read_to_string(path)?.lines().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(text)
                .map_err(|error| format!("Replay line {}: {error}", line + 1))?;
            if record["phase"] != "begin" {
                continue;
            }
            seed = record["seed"].as_u64().unwrap_or(seed);
            let action = serde_json::from_value(record["action"].clone())
                .map_err(|error| format!("Replay action on line {}: {error}", line + 1))?;
            actions.push_back(Recorded {
                action,
                cleanup: record["cleanup"].as_bool().unwrap_or(false),
            });
        }
        if actions.is_empty() {
            return Err("The monkey journal contains no recorded actions".into());
        }
        let mut runner = Self::new(seed, budget, Some(actions.len() as u64), output)?;
        runner.replay = Some(actions);
        Ok(runner)
    }

    /// Execute one action at a frame boundary and dispatch ordinary widget actions.
    #[allow(
        dead_code,
        reason = "Retained-input consumers share the same durable runner without a platform router"
    )]
    pub fn step(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        self.step_with(editor, shell, Action::run)
    }

    /// Run the recorded action through the production input owner. The journal,
    /// watchdog, panic boundary and counts include the callback's complete work.
    pub fn step_with(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        mut run: impl FnMut(&Action, &mut Editor, &mut Shell, &mut dyn Clipboard) -> ara_ui::Result<()>,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        if !shell.offscreen {
            return Err("the editor monkey requires an offscreen run".into());
        }
        if self.actions == 0
            && !editor.host.root().join(".ara-monkey-fixture").is_file()
            && !editor.host.root().join(".ara-behaviour-fixture").is_file()
        {
            return Err("The editor monkey requires a fresh scratch project marked .ara-monkey-fixture or .ara-behaviour-fixture".into());
        }
        shell.open_ide = false;
        if self.actions == 0 {
            self.read_diagnostics(editor)?;
            self.baseline_diagnostics = std::mem::take(&mut self.diagnostics);
        } else {
            // A native click or registered Clear action can erase diagnostics.
            // Observe them before every action, not only each report interval.
            self.read_diagnostics(editor)?;
        }
        let Recorded { action, cleanup } = if let Some(replay) = &mut self.replay {
            let Some(action) = replay.pop_front() else {
                self.stopped = true;
                return Ok(false);
            };
            action
        } else if self.budget_reached() {
            let since = self.draining_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= DRAIN {
                self.cleanup_incomplete = true;
                self.stopped = true;
                self.write_event(json!({"phase":"cleanupIncomplete","step":self.actions,"playResources":self.play.report(),"heldKeys":self.held_keys,"heldButtons":self.held_buttons}))?;
                self.write_report()?;
                return Ok(false);
            }
            let action = if !self.held_buttons.is_empty() || !self.held_keys.is_empty() {
                Action::FocusLost
            } else if let Some(action) = self.play.drain() {
                action
            } else {
                return Ok(false);
            };
            self.drain_actions += 1;
            Recorded {
                action,
                cleanup: true,
            }
        } else {
            let generator = &mut self.generator;
            let before = self.play.cleanup_actions();
            let action = self.play.next(|| {
                generator.initialize(editor);
                generator.next(shell)
            });
            Recorded {
                action,
                cleanup: self.play.cleanup_actions() != before,
            }
        };
        let action_json = serde_json::to_value(&action)?;
        self.last_action = Some(action_json.clone());
        self.watchdog
            .progress(self.actions, Some(action_json.clone()));
        self.write_event(
            json!({"phase":"begin","seed":self.seed,"step":self.actions,"cleanup":cleanup,"action":action_json}),
        )?;
        let at = Instant::now();
        let mut play_event = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let before = self.play.before(editor)?;
            let result = run(&action, editor, shell, &mut self.clipboard);
            play_event = self.play.after(editor, &before)?;
            result
        }));
        self.actions += 1;
        if cleanup {
            self.cleanup_actions += 1;
        } else {
            self.workload_actions += 1;
        }
        self.track_held(&action);
        let accepted = matches!(&result, Ok(Ok(())));
        let outcome = match result {
            Ok(Ok(())) => "accepted".to_owned(),
            Ok(Err(error)) => {
                self.rejected += 1;
                format!("rejected: {error}")
            }
            Err(payload) => {
                let message = panic_message(&*payload);
                self.panics
                    .push(json!({"step":self.actions-1,"action":action_json,"message":message}));
                // A command panic may have partially mutated authority state. Record it
                // and end this seed; continuing would hide the original failure.
                self.stopped = true;
                format!("panic: {message}")
            }
        };
        match &action {
            Action::Command { id, target } => {
                *self.commands.entry(id.clone()).or_default() += 1;
                if accepted {
                    *self.command_successes.entry(id.clone()).or_default() += 1;
                }
                if let Some(case) = actions::edge_case(target) {
                    self.command_cases
                        .entry(id.clone())
                        .or_default()
                        .insert(case);
                }
            }
            Action::Protocol { method, .. } => {
                *self.methods.entry(method.clone()).or_default() += 1
            }
            _ => {
                if let Some(kind) = action_json["kind"].as_str() {
                    *self.ui_actions.entry(kind.into()).or_default() += 1;
                }
            }
        }
        // Window commands are exercised through the real shell, but their platform
        // request must never display a file dialog or terminate the soak process.
        if let Some(request) = shell.window_request.take() {
            self.write_event(json!({"phase":"platformRequest","step":self.actions-1,"request":format!("{request:?}")}))?;
        }
        self.boundary_failures = self.boundary_failures.max(shell.panel_failure_count());
        if let Some(inventory) = play_event {
            self.write_event(
                json!({"phase":"playInventory","step":self.actions-1,"inventory":inventory}),
            )?;
        }
        self.write_event(json!({"phase":"end","step":self.actions-1,"outcome":outcome,"actionMs":at.elapsed().as_secs_f64()*1000.}))?;
        if self.actions.is_multiple_of(64) || self.finished() {
            self.read_diagnostics(editor)?;
            self.write_report()?;
        }
        Ok(true)
    }

    /// Record the complete production-loop frame supplied by the window owner.
    pub fn record_frame(&mut self, elapsed: Duration) {
        self.frames += 1;
        self.watchdog.progress(self.actions, None);
        let ms = elapsed.as_secs_f64() * 1000.;
        self.max_frame_ms = self.max_frame_ms.max(ms);
        if elapsed >= SPIKE {
            self.spikes += 1;
            self.slowest.push(json!({"frame":self.frames,"step":self.actions.saturating_sub(1),"milliseconds":ms,"action":self.last_action}));
            self.slowest.sort_by(|a, b| {
                b["milliseconds"]
                    .as_f64()
                    .unwrap_or(0.)
                    .total_cmp(&a["milliseconds"].as_f64().unwrap_or(0.))
            });
            self.slowest.truncate(32);
        }
        if elapsed >= HANG {
            self.hangs += 1;
        }
    }

    /// Generated budgets finish after their bounded recorded resource drain;
    /// replay exhaustion and panics stop immediately without inferred actions.
    pub fn finished(&self) -> bool {
        self.stopped
            || if let Some(replay) = &self.replay {
                self.started.elapsed() >= self.budget || replay.is_empty()
            } else {
                self.budget_reached() && self.resources_drained()
            }
    }

    fn budget_reached(&self) -> bool {
        self.started.elapsed() >= self.budget
            || self.max_steps.is_some_and(|n| self.workload_actions >= n)
    }
    fn resources_drained(&self) -> bool {
        self.play.drained() && self.held_buttons.is_empty() && self.held_keys.is_empty()
    }
    fn track_held(&mut self, action: &Action) {
        match action {
            Action::Press { .. } => {
                self.held_buttons.insert(1);
            }
            Action::Click { .. } | Action::Release { .. } => {
                self.held_buttons.remove(&1);
            }
            Action::PointerButton {
                button, pressed, ..
            } => {
                if *pressed {
                    self.held_buttons.insert(*button);
                } else {
                    self.held_buttons.remove(button);
                }
            }
            Action::KeyState { scancode, pressed } => {
                if *pressed {
                    self.held_keys.insert(*scancode);
                } else {
                    self.held_keys.remove(scancode);
                }
            }
            Action::Key { scancode } => {
                self.held_keys.remove(scancode);
            }
            Action::FocusLost => {
                self.held_buttons.clear();
                self.held_keys.clear();
            }
            _ => {}
        }
    }

    /// Bounded acceptance requires complete diagnostics, drained resources and
    /// no crashes, unresponsive frames or quarantined panels.
    pub fn succeeded(&self) -> bool {
        self.finished()
            && self.panics.is_empty()
            && self.hangs.max(self.watchdog.count()) == 0
            && self.boundary_failures == 0
            && !self.cleanup_incomplete
            && self.resources_drained()
            && self.replay.as_ref().is_none_or(VecDeque::is_empty)
            && self.final_diagnostics
            && self.diagnostics_truncated == 0
            && !self.diagnostic_bus_loss
    }

    /// A duration-only generated run of at least thirty minutes is a soak;
    /// explicit action caps and exact journal replays are bounded checks instead.
    pub fn is_soak(&self) -> bool {
        self.replay.is_none()
            && self.max_steps.is_none()
            && self.budget >= Duration::from_secs(1800)
    }

    /// Long-run acceptance additionally requires every registered command's edge
    /// targets and actual events through the native production router.
    pub fn soak_succeeded(&self) -> bool {
        self.is_soak()
            && self.succeeded()
            && self.started.elapsed() >= self.budget
            && self.native_events > 0
            && self.missing_command_cases().is_empty()
    }

    /// Count events actually offered to route_event, rather than generated events
    /// that an earlier rejection prevented from reaching the production owner.
    pub fn record_native_events(&mut self, events: u64) {
        self.native_events += events;
    }

    /// Persist the final summary even when an enclosing frame limit ended the run.
    pub fn finish(&mut self) -> ara_ui::Result<()> {
        self.journal.flush()?;
        self.write_report()
    }

    /// Drain the last diagnostics tail even if the duration elapsed between
    /// actions, then write the authoritative final report.
    pub fn finish_with(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.watchdog.progress(self.actions, None);
        self.read_diagnostics(editor)?;
        self.final_diagnostics = true;
        self.finish()
    }

    fn write_event(&mut self, event: Value) -> ara_ui::Result<()> {
        // JSONL frames use the same compact serializer as the host journal;
        // standalone report documents use Ara's canonical document writer.
        let line = serde_json::to_string(&event)?;
        self.journal.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            self.journal.write_all(b"\n")?;
        }
        self.journal.flush()?;
        Ok(())
    }

    fn read_diagnostics(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let logs = editor.action(
            "logs",
            json!({"since":self.log_cursor,"severity":"warning","group":false,"limit":1000}),
        )?;
        let returned = logs["entries"].as_array().map_or(0, Vec::len) as u64;
        let total = logs["total"].as_u64().unwrap_or(returned);
        if total > returned {
            self.diagnostics_truncated += total - returned;
            self.write_event(json!({"phase":"diagnosticsTruncated","step":self.actions,"totalRecords":total,"returnedRecords":returned}))?;
        }
        if let Some(cursor) = logs["cursor"].as_u64() {
            self.log_cursor = cursor;
        }
        for entry in logs["entries"].as_array().into_iter().flatten() {
            let code = entry["diagnostic"]["code"].as_str().unwrap_or("unknown");
            let first = entry["first"].as_u64().unwrap_or(0);
            let count = entry["count"].as_u64().unwrap_or(1);
            let previous = self.diagnostic_counts.insert(first, count).unwrap_or(0);
            if ara_diag::Code::parse(code) == Some(ara_diag::codes::DIAG_DROPPED) {
                self.diagnostic_bus_loss = true;
                self.write_event(
                    json!({"phase":"diagnosticBusLoss","step":self.actions,"entry":entry}),
                )?;
            }
            if entry["diagnostic"]["severity"] == "error" {
                *self.diagnostics.entry(code.to_owned()).or_default() +=
                    count.saturating_sub(previous);
                self.write_event(json!({"phase":"diagnostic","step":self.actions,"entry":entry}))?;
            }
        }
        Ok(())
    }

    fn write_report(&self) -> ara_ui::Result<()> {
        let unvisited: Vec<_> = commands::all()
            .iter()
            .filter(|spec| !self.commands.contains_key(spec.id))
            .map(|spec| spec.id)
            .collect();
        let missing_cases = self.missing_command_cases();
        let accepted = if self.is_soak() {
            self.soak_succeeded()
        } else {
            self.succeeded()
        };
        let report = json!({
            "seed":self.seed,"finished":self.finished(),"accepted":accepted,
            "seconds":self.started.elapsed().as_secs_f64(),"requestedSeconds":self.budget.as_secs_f64(),
            "soakRequested":self.is_soak(),"soakAccepted":self.soak_succeeded(),
            "durationCompleted":self.started.elapsed()>=self.budget,
            "termination":self.termination(),
            "actions":self.actions,"frames":self.frames,"rejectedInputs":self.rejected,
            "requestedSteps":self.max_steps,"workloadActions":self.workload_actions,"cleanupActions":self.cleanup_actions,
            "cleanupIncomplete":self.cleanup_incomplete||!self.resources_drained(),
            "endDrain":{"actions":self.drain_actions,"limitSeconds":DRAIN.as_secs(),"seconds":self.draining_since.map(|since|since.elapsed().as_secs_f64()),
                "cleanupIncomplete":self.cleanup_incomplete||!self.resources_drained(),"heldKeys":self.held_keys,"heldButtons":self.held_buttons},
            "panics":self.panics,"hangs":self.hangs.max(self.watchdog.count()),"hangThresholdMs":HANG.as_millis(),
            "longFrames":self.hangs,"watchdogStalls":self.watchdog.count(),
            "frameSpikes":self.spikes,"spikeThresholdMs":SPIKE.as_millis(),"maxFrameMs":self.max_frame_ms,
            "slowestFrames":self.slowest,"panelFailures":self.boundary_failures,
            "errorDiagnosticsBefore":self.baseline_diagnostics,"errorDiagnosticsDuring":self.diagnostics,"registryCommands":self.commands,
            "baselineDiagnosticsCollected":self.actions>0,"diagnosticBusLoss":self.diagnostic_bus_loss,
            "finalDiagnosticsCollected":self.final_diagnostics,"diagnosticRecordsOmitted":self.diagnostics_truncated,
            "commandEdgeCases":self.command_cases,"commandCallbacksAccepted":self.command_successes,
            "protocolMethods":self.methods,"unvisitedCommands":unvisited,
            "uiActions":self.ui_actions,
            "coverageComplete":missing_cases.is_empty(),"missingCommandEdgeCases":missing_cases,
            "edgeCasesPerCommand":actions::edge_case_count(),"nativeEventsRouted":self.native_events,
            "journal":"monkey-events.jsonl",
            "replayedJournal":self.replay.is_some(),
            "replayRemaining":self.replay.as_ref().map(VecDeque::len),
            "playResources":self.play.report(),
            "watchdog":"monkey-watchdog.json",
            "replay":format!("--verify-monkey --monkey-seed {} --monkey-seconds {} --monkey-steps {}",self.seed,self.budget.as_secs(),self.workload_actions.max(1)),
            "exactReplay":"--monkey-replay <original-output>/monkey-events.jsonl --output <new-output>",
            "limits":["Command coverage counts attempted edge targets; disabled or deferred commands can reject before their implementation runs. Accepted callbacks are counted separately.","Protocol method probes include malformed envelopes and stale sessions; a method attempt is not complete implementation coverage.","The watchdog persists permanent stalls independently; an external process timeout is required to end a permanently stalled editor.","Long frames and watchdog stalls are reported separately; hangs is their lower bound because their intervals can overlap.","NaN and infinity reach numeric controls as text because JSON cannot represent non-finite numbers."]
        });
        std::fs::write(
            self.output.join("monkey.json"),
            ara_json::to_canonical_string(&report),
        )?;
        Ok(())
    }

    fn missing_command_cases(&self) -> Vec<Value> {
        commands::all()
            .iter()
            .filter_map(|command| {
                let seen = self.command_cases.get(command.id);
                let missing: Vec<_> = (0..actions::edge_case_count())
                    .filter(|case| seen.is_none_or(|seen| !seen.contains(case)))
                    .collect();
                if missing.is_empty() {
                    None
                } else {
                    Some(json!({"command":command.id,"cases":missing}))
                }
            })
            .collect()
    }

    fn termination(&self) -> &'static str {
        if !self.panics.is_empty() {
            "panic"
        } else if self.cleanup_incomplete {
            "cleanup-timeout"
        } else if self.replay.as_ref().is_some_and(VecDeque::is_empty) {
            "replay-exhausted"
        } else if self
            .max_steps
            .is_some_and(|steps| self.workload_actions >= steps)
        {
            "workload-step-limit"
        } else if self.started.elapsed() >= self.budget {
            "elapsed-duration"
        } else if self.finished() {
            "stopped"
        } else {
            "incomplete"
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panic with a non-text payload")
}
