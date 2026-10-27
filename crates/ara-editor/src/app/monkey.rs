//! Seeded editor soak: real retained input, registry commands and protocol validation.
//!
//! Every action is flushed before it runs. If the process stops responding, the last
//! `begin` without an `end` identifies the exact action, seed and sequence to replay.
mod actions;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod watchdog;

use crate::{
    Editor,
    shell::{Shell, commands},
};
use actions::{Action, Generator, MemoryClipboard};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{BufWriter, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SPIKE: Duration = Duration::from_millis(100);
const HANG: Duration = Duration::from_secs(10);

/// One production-loop runner. No platform windows or global input are synthesized.
pub(super) struct Monkey {
    seed: u64,
    budget: Duration,
    max_steps: Option<u64>,
    started: Instant,
    output: PathBuf,
    journal: BufWriter<File>,
    generator: Generator,
    replay: Option<VecDeque<Action>>,
    clipboard: MemoryClipboard,
    actions: u64,
    frames: u64,
    rejected: u64,
    panics: Vec<Value>,
    hangs: u64,
    spikes: u64,
    max_frame_ms: f64,
    slowest: Vec<Value>,
    commands: BTreeMap<String, u64>,
    methods: BTreeMap<String, u64>,
    diagnostics: BTreeMap<String, u64>,
    baseline_diagnostics: BTreeMap<String, u64>,
    diagnostic_counts: BTreeMap<u64, u64>,
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
            actions: 0,
            frames: 0,
            rejected: 0,
            panics: Vec::new(),
            hangs: 0,
            spikes: 0,
            max_frame_ms: 0.,
            slowest: Vec::new(),
            commands: BTreeMap::new(),
            methods: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            baseline_diagnostics: BTreeMap::new(),
            diagnostic_counts: BTreeMap::new(),
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
            actions.push_back(action);
        }
        if actions.is_empty() {
            return Err("The monkey journal contains no recorded actions".into());
        }
        let mut runner = Self::new(seed, budget, Some(actions.len() as u64), output)?;
        runner.replay = Some(actions);
        Ok(runner)
    }

    /// Execute one action at a frame boundary and dispatch ordinary widget actions.
    pub fn step(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        if !shell.offscreen {
            return Err("the editor monkey requires an offscreen run".into());
        }
        shell.open_ide = false;
        if self.actions == 0 {
            self.read_diagnostics(editor)?;
            self.baseline_diagnostics = std::mem::take(&mut self.diagnostics);
        }
        let action = if let Some(replay) = &mut self.replay {
            let Some(action) = replay.pop_front() else {
                self.stopped = true;
                return Ok(false);
            };
            action
        } else {
            self.generator.initialize(editor);
            self.generator.next(shell)
        };
        let action_json = serde_json::to_value(&action)?;
        self.last_action = Some(action_json.clone());
        self.watchdog
            .progress(self.actions, Some(action_json.clone()));
        self.write_event(
            json!({"phase":"begin","seed":self.seed,"step":self.actions,"action":action_json}),
        )?;
        let at = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            action.run(editor, shell, &mut self.clipboard)
        }));
        self.actions += 1;
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
            Action::Command { id, .. } => *self.commands.entry(id.clone()).or_default() += 1,
            Action::Protocol { method, .. } => {
                *self.methods.entry(method.clone()).or_default() += 1
            }
            _ => {}
        }
        // Window commands are exercised through the real shell, but their platform
        // request must never display a file dialog or terminate the soak process.
        if let Some(request) = shell.window_request.take() {
            self.write_event(json!({"phase":"platformRequest","step":self.actions-1,"request":format!("{request:?}")}))?;
        }
        self.boundary_failures = self.boundary_failures.max(shell.panel_failure_count());
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

    /// A time/action budget ends a run; a panic ends it immediately with failure.
    pub fn finished(&self) -> bool {
        self.stopped
            || self.started.elapsed() >= self.budget
            || self.max_steps.is_some_and(|n| self.actions >= n)
    }

    /// Acceptance concerns crashes, unresponsive frames and quarantined panels.
    pub fn succeeded(&self) -> bool {
        self.finished()
            && self.panics.is_empty()
            && self.hangs.max(self.watchdog.count()) == 0
            && self.boundary_failures == 0
            && self.replay.as_ref().is_none_or(VecDeque::is_empty)
    }

    /// Persist the final summary even when an enclosing frame limit ended the run.
    pub fn finish(&mut self) -> ara_ui::Result<()> {
        self.journal.flush()?;
        self.write_report()
    }

    fn write_event(&mut self, event: Value) -> ara_ui::Result<()> {
        let line = ara_json::to_canonical_string(&event);
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
            json!({"since":self.log_cursor,"severity":"error","group":false,"limit":1000}),
        )?;
        if let Some(cursor) = logs["cursor"].as_u64() {
            self.log_cursor = cursor;
        }
        for entry in logs["entries"].as_array().into_iter().flatten() {
            let code = entry["diagnostic"]["code"].as_str().unwrap_or("unknown");
            let first = entry["first"].as_u64().unwrap_or(0);
            let count = entry["count"].as_u64().unwrap_or(1);
            let previous = self.diagnostic_counts.insert(first, count).unwrap_or(0);
            *self.diagnostics.entry(code.to_owned()).or_default() += count.saturating_sub(previous);
            self.write_event(json!({"phase":"diagnostic","step":self.actions,"entry":entry}))?;
        }
        Ok(())
    }

    fn write_report(&self) -> ara_ui::Result<()> {
        let unvisited: Vec<_> = commands::all()
            .iter()
            .filter(|spec| !self.commands.contains_key(spec.id))
            .map(|spec| spec.id)
            .collect();
        let report = json!({
            "seed":self.seed,"finished":self.finished(),"accepted":self.succeeded(),
            "seconds":self.started.elapsed().as_secs_f64(),"requestedSeconds":self.budget.as_secs_f64(),
            "actions":self.actions,"frames":self.frames,"rejectedInputs":self.rejected,
            "panics":self.panics,"hangs":self.hangs.max(self.watchdog.count()),"hangThresholdMs":HANG.as_millis(),
            "frameSpikes":self.spikes,"spikeThresholdMs":SPIKE.as_millis(),"maxFrameMs":self.max_frame_ms,
            "slowestFrames":self.slowest,"panelFailures":self.boundary_failures,
            "errorDiagnosticsBefore":self.baseline_diagnostics,"errorDiagnosticsDuring":self.diagnostics,"registryCommands":self.commands,
            "protocolMethods":self.methods,"unvisitedCommands":unvisited,
            "coverageComplete":unvisited.is_empty(),
            "journal":"monkey-events.jsonl",
            "replayedJournal":self.replay.is_some(),
            "replayRemaining":self.replay.as_ref().map(VecDeque::len),
            "watchdog":"monkey-watchdog.json",
            "replay":format!("--verify-monkey --monkey-seed {} --monkey-seconds {} --monkey-steps {}",self.seed,self.budget.as_secs(),self.actions.max(1)),
            "exactReplay":"--monkey-replay <original-output>/monkey-events.jsonl --output <new-output>",
            "limits":["The watchdog persists permanent stalls independently; an external process timeout is required to end a permanently stalled editor.","NaN and infinity reach numeric controls as text because JSON cannot represent non-finite numbers."]
        });
        std::fs::write(
            self.output.join("monkey.json"),
            ara_json::to_canonical_string(&report),
        )?;
        Ok(())
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panic with a non-text payload")
}
