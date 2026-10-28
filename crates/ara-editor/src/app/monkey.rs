//! Seeded editor soak: real retained input, registry commands and protocol validation.
//!
//! Every action is flushed before it runs. If the process stops responding, the last
//! `begin` without an `end` identifies the exact action, seed and sequence to replay.
mod actions;
mod catalog;
mod journal;
mod measurements;
pub(super) mod native;
mod output_health;
mod play;
mod report_writer;
mod resources;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod typed;
mod watchdog;

use crate::{Editor, shell::Shell};
pub(super) use actions::Action;
use actions::{Generator, MemoryClipboard};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SPIKE: Duration = Duration::from_millis(100);
const HANG: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(30);
const JOURNAL_ACK: Duration = Duration::from_secs(120);

struct Recorded {
    action: Action,
    cleanup: bool,
}
struct PendingAction {
    recorded: Recorded,
    json: Value,
    ticket: journal::Ticket,
    since: Instant,
}

/// One production-loop runner with durable actions and an injectable input router.
pub(super) struct Monkey {
    seed: u64,
    host_seed: Option<u64>,
    replay_original_host_seed: Option<u64>,
    budget: Duration,
    max_steps: Option<u64>,
    started: Instant,
    output: PathBuf,
    journal: journal::Writer,
    pending_action: Option<PendingAction>,
    journal_error: Option<String>,
    aborted_action: Option<Value>,
    journal_wait_frames: u64,
    journal_wait_ms: f64,
    max_journal_wait_ms: f64,
    baseline_collected: bool,
    last_wait_report: Instant,
    reports: report_writer::Reports,
    report_io_failed: bool,
    generator: Generator,
    replay: Option<VecDeque<Recorded>>,
    clipboard: MemoryClipboard,
    play: play::State,
    typed: typed::State,
    shell_catalog: catalog::State,
    resources: resources::Resources,
    last_catalog_matched: Option<bool>,
    actions: u64,
    workload_actions: u64,
    cleanup_actions: u64,
    drain_actions: u64,
    draining_since: Option<Instant>,
    cleanup_incomplete: bool,
    held_buttons: BTreeSet<u8>,
    held_keys: BTreeSet<i32>,
    frames: u64,
    offscreen_output: Option<bool>,
    gpu_surface_submissions: u64,
    native_presentations: u64,
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
    measurements: measurements::Measurements,
    output_health: output_health::Health,
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
            host_seed: None,
            replay_original_host_seed: None,
            budget,
            max_steps,
            started: Instant::now(),
            output: output.into(),
            journal: journal::Writer::new(&output.join("monkey-events.jsonl"))?,
            pending_action: None,
            journal_error: None,
            aborted_action: None,
            journal_wait_frames: 0,
            journal_wait_ms: 0.,
            max_journal_wait_ms: 0.,
            baseline_collected: false,
            last_wait_report: Instant::now(),
            reports: report_writer::Reports::new(output.join("monkey.json"))?,
            report_io_failed: false,
            generator: Generator::new(seed),
            replay: None,
            clipboard: MemoryClipboard::default(),
            play: play::State::default(),
            typed: typed::State::default(),
            shell_catalog: catalog::State::default(),
            resources: resources::Resources::default(),
            last_catalog_matched: None,
            actions: 0,
            workload_actions: 0,
            cleanup_actions: 0,
            drain_actions: 0,
            draining_since: None,
            cleanup_incomplete: false,
            held_buttons: BTreeSet::new(),
            held_keys: BTreeSet::new(),
            frames: 0,
            offscreen_output: None,
            gpu_surface_submissions: 0,
            native_presentations: 0,
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
            measurements: measurements::Measurements::default(),
            output_health: output_health::Health::new(HANG),
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
        let replay = journal::read(path)?;
        let mut runner = Self::new(
            replay.seed,
            budget,
            Some(replay.actions.len() as u64),
            output,
        )?;
        runner.replay_original_host_seed = replay.original_host_seed;
        runner.replay = Some(replay.actions);
        Ok(runner)
    }

    /// Read the Host seed before constructing the replay's Editor. Legacy journals
    /// fall back to the action seed; their original Host entropy remains unknown.
    pub fn journal_seed(path: &Path) -> ara_ui::Result<u64> {
        journal::seed(path)
    }

    /// Record the exact seed supplied to HostConfig at startup, before any action.
    /// Other consumers retain unknown provenance rather than inventing a Host seed.
    pub fn record_host_seed(&mut self, seed: u64) -> ara_ui::Result<()> {
        if self.actions != 0 || self.pending_action.is_some() || seed != self.seed {
            return Err(
                "Host seed must match the monkey seed and be recorded before actions".into(),
            );
        }
        self.host_seed = Some(seed);
        Ok(())
    }

    /// Bounded CPU-test pump through the same acknowledgement path. Production
    /// native consumers call step_with once per frame and never wait here.
    #[cfg(test)]
    pub fn step(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        let before = self.actions;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let changed = self.step_with(editor, shell, Action::run)?;
            if self.actions > before || self.finished() || self.pending_action.is_none() {
                return Ok(changed);
            }
            if Instant::now() >= deadline {
                return Err("CPU test journal acknowledgement exceeded five seconds".into());
            }
            editor.tick();
            std::thread::yield_now();
        }
    }

    /// Run the recorded action through the production input owner. The journal,
    /// watchdog, panic boundary and counts include the callback's complete work.
    pub fn step_with(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        mut run: impl FnMut(&Action, &mut Editor, &mut Shell, &mut dyn Clipboard) -> ara_ui::Result<()>,
    ) -> ara_ui::Result<bool> {
        self.measurements.begin_step();
        if self.finished() {
            return Ok(false);
        }
        if !shell.offscreen {
            return Err("the editor monkey requires an offscreen run".into());
        }
        if !self.baseline_collected
            && !editor.host.root().join(".ara-monkey-fixture").is_file()
            && !editor.host.root().join(".ara-behaviour-fixture").is_file()
        {
            return Err("The editor monkey requires a fresh scratch project marked .ara-monkey-fixture or .ara-behaviour-fixture".into());
        }
        shell.open_ide = false;
        self.shell_catalog.observe(&editor.extensions);
        if !self.resources.initialized() {
            // Instrumentation precedes every workload action, including queued
            // fixture actions. It never borrows or clones authoring documents.
            let inventory = editor.action("jobs.list", json!({}))?;
            self.resources.baseline(&editor.session, &inventory)?;
            self.typed.baseline_jobs(&inventory)?;
            self.write_event(json!({"phase":"jobInventory","when":"startup","session":editor.session,"jobs":self.resources.report()["baselineJobs"]}))?;
        }
        if !self.baseline_collected {
            self.read_diagnostics(editor)?;
            self.baseline_diagnostics = std::mem::take(&mut self.diagnostics);
            self.baseline_collected = true;
        } else {
            // A native click or registered Clear action can erase diagnostics.
            // Observe them before every action, not only each report interval.
            self.read_diagnostics(editor)?;
        }
        if self.journal_error.is_some() {
            return Ok(false);
        }
        if self.pending_action.is_none() {
            let recorded = if let Some(replay) = &mut self.replay {
                let Some(action) = replay.pop_front() else {
                    // Replay preserves actions exactly, but asynchronous effects
                    // may retire later than in the original run. Pump normally
                    // and observe them; never synthesize cleanup commands.
                    shell.cancel_script_module_intent();
                    editor.probe_captures.begin_drain();
                    let since = self.draining_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= DRAIN {
                        self.cleanup_incomplete = true;
                        self.stopped = true;
                    }
                    self.play.poll(editor)?;
                    self.resources.shell(background_work(editor, shell));
                    self.refresh_end_inventory(editor)?;
                    return Ok(false);
                };
                action
            } else if self.budget_reached() {
                // Cancelling presentation never pretends discovery has retired;
                // its worker remains part of the resource drain until joined.
                shell.cancel_script_module_intent();
                editor.probe_captures.begin_drain();
                if self.is_soak() {
                    self.shell_catalog.begin_drain();
                }
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
                } else if let Some(action) = self
                    .play
                    .drain()
                    .or_else(|| self.typed.drain())
                    .or_else(|| self.resources.next())
                    .or_else(|| {
                        if self.is_soak() {
                            self.shell_catalog.next()
                        } else {
                            None
                        }
                    })
                {
                    action
                } else if self.is_soak() && self.shell_catalog.drain_pending() {
                    Action::Protocol {
                        method: "editor.commands".into(),
                        params: json!({}),
                        raw: false,
                    }
                } else {
                    return Ok(false);
                };
                Recorded {
                    action,
                    cleanup: true,
                }
            } else if let Some(action) = self
                .typed
                .discovery_action()
                .filter(|_| !self.generator.queued())
            {
                Recorded {
                    action,
                    cleanup: false,
                }
            } else {
                let generator = &mut self.generator;
                let typed = &mut self.typed;
                let catalog = &mut self.shell_catalog;
                let before = self.play.cleanup_actions();
                let action = self.play.next(|| {
                    generator.initialize(editor);
                    generator.next(editor, shell, typed, catalog)
                });
                Recorded {
                    action,
                    cleanup: self.play.cleanup_actions() != before,
                }
            };
            let action_json = serde_json::to_value(&recorded.action)?;
            let line = self.journal_line(json!({"phase":"begin","seed":self.seed,"hostSeed":self.host_seed,
            "step":self.actions,"ticket":self.journal.next_ticket(),"cleanup":recorded.cleanup,"action":action_json}))?;
            let at = Instant::now();
            let ticket = self.journal.begin(line);
            self.measurements
                .record(measurements::Stage::JournalSubmit, at.elapsed());
            match ticket {
                Ok(ticket) => {
                    self.pending_action = Some(PendingAction {
                        recorded,
                        json: action_json,
                        ticket,
                        since: Instant::now(),
                    })
                }
                Err(error) => {
                    self.fail_journal(error.to_string());
                    return Ok(false);
                }
            }
        }
        let ticket = self
            .pending_action
            .as_ref()
            .ok_or("Journal action staging disappeared")?
            .ticket;
        let at = Instant::now();
        let acknowledged = self.journal.poll(ticket);
        self.measurements
            .record(measurements::Stage::JournalSubmit, at.elapsed());
        if acknowledged.is_ok()
            && self
                .pending_action
                .as_ref()
                .is_some_and(|pending| pending.recorded.cleanup)
            && self
                .draining_since
                .is_some_and(|since| since.elapsed() >= DRAIN)
        {
            self.cleanup_incomplete = true;
            self.stopped = true;
            self.abort_pending(
                "Resource drain deadline elapsed before this cleanup action was dispatched",
            )?;
            self.write_event(json!({"phase":"cleanupIncomplete","step":self.actions,"reason":"journal acknowledgement pending at resource drain deadline"}))?;
            self.write_report()?;
            return Ok(false);
        }
        match acknowledged {
            Ok(false) => {
                self.journal_wait_frames += 1;
                if self
                    .pending_action
                    .as_ref()
                    .is_some_and(|pending| pending.since.elapsed() >= JOURNAL_ACK)
                {
                    self.fail_journal(format!("Journal begin acknowledgement exceeded {} seconds; native loop continued while waiting",JOURNAL_ACK.as_secs()));
                }
                if self.journal_error.is_some()
                    || self.last_wait_report.elapsed() >= Duration::from_secs(1)
                {
                    self.write_report()?;
                    self.last_wait_report = Instant::now();
                }
                return Ok(false);
            }
            Err(error) => {
                self.fail_journal(error.to_string());
                return Ok(false);
            }
            Ok(true) => {}
        }
        let pending = self
            .pending_action
            .take()
            .ok_or("Acknowledged journal action disappeared")?;
        self.record_journal_wait(pending.since.elapsed());
        let Recorded { action, cleanup } = pending.recorded;
        let action_json = pending.json;
        self.last_action = Some(action_json.clone());
        self.watchdog
            .progress(self.actions, Some(action_json.clone()));
        let at = Instant::now();
        let mut play_event = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let before = self.play.before(editor)?;
            let result = match self.typed.run(&action, editor, shell) {
                Some(result) => result,
                None => run(&action, editor, shell, &mut self.clipboard),
            };
            play_event = self.play.after(editor, &before)?;
            result
        }));
        self.actions += 1;
        if cleanup && self.draining_since.is_some() {
            self.drain_actions += 1;
        }
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
            Action::TypedCommand { .. } => {
                *self.methods.entry("cmd.apply".into()).or_default() += 1;
            }
            Action::Command { id, target } => {
                *self.commands.entry(id.clone()).or_default() += 1;
                if accepted {
                    *self.command_successes.entry(id.clone()).or_default() += 1;
                }
                if let Some(case) = actions::edge_case(target) {
                    self.shell_catalog.record(id, case);
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
        if let Some(catalog) = self.typed.take_catalog() {
            std::fs::write(
                self.output.join("monkey-typed-catalog.json"),
                ara_json::to_canonical_string(&catalog),
            )?;
            self.write_event(json!({"phase":"typedCatalog","step":self.actions-1,"catalog":"monkey-typed-catalog.json","names":catalog["names"],"errors":catalog["errors"]}))?;
        }
        for result in self.typed.take_events() {
            self.write_event(json!({"phase":"typedResult","step":self.actions-1,"result":result}))?;
        }
        self.write_shell_catalog_events()?;
        if self.panics.is_empty() {
            self.observe_shell_resources(editor, shell)?;
        }
        self.write_event(json!({"phase":"end","step":self.actions-1,"outcome":outcome,"actionMs":at.elapsed().as_secs_f64()*1000.}))?;
        if self.actions.is_multiple_of(64) || self.finished() {
            self.read_diagnostics(editor)?;
            self.write_report()?;
        }
        Ok(true)
    }

    /// Record the complete production-loop frame supplied by the window owner.
    #[allow(dead_code, reason = "CPU/replay consumers retain the frame-only API")]
    pub fn record_frame(&mut self, elapsed: Duration) {
        self.record_frame_inner(elapsed, None);
    }

    /// Attach production phase timings only to slow frames; normal frames allocate
    /// no stage JSON and use the same counters as [`Self::record_frame`].
    pub fn record_frame_with_stages(
        &mut self,
        elapsed: Duration,
        stages: &super::responsiveness::Sample,
    ) {
        self.record_frame_inner(elapsed, Some(stages));
    }

    fn record_frame_inner(
        &mut self,
        elapsed: Duration,
        stages: Option<&super::responsiveness::Sample>,
    ) {
        self.frames += 1;
        self.output_health.record(
            elapsed,
            stages.map(|sample| output_health::Sample {
                requested: sample.output_requested,
                submissions: sample.gpu_surface_submissions,
                presentations: sample.native_presentations,
                offscreen: sample.offscreen_output,
            }),
        );
        if let Some(stages) = stages {
            self.offscreen_output = Some(stages.offscreen_output);
            self.gpu_surface_submissions += u64::from(stages.gpu_surface_submissions);
            self.native_presentations += u64::from(stages.native_presentations);
        }
        self.watchdog.progress(self.actions, None);
        let ms = elapsed.as_secs_f64() * 1000.;
        self.max_frame_ms = self.max_frame_ms.max(ms);
        if elapsed >= SPIKE {
            self.spikes += 1;
            self.slowest.push(json!({"frame":self.frames,"step":self.actions.saturating_sub(1),"milliseconds":ms,"action":self.last_action,"stages":stages,"harnessStages":self.measurements.step()}));
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
    /// replay exhaustion waits for existing work without inferring extra actions.
    pub fn finished(&self) -> bool {
        if self.pending_action.is_some() {
            return false;
        }
        self.stopped
            || if let Some(replay) = &self.replay {
                self.started.elapsed() >= self.budget
                    || (replay.is_empty() && self.resources_drained())
            } else {
                self.budget_reached()
                    && self.resources_drained()
                    && (!self.is_soak() || !self.shell_catalog.drain_pending())
            }
    }

    fn budget_reached(&self) -> bool {
        self.started.elapsed() >= self.budget
            || self.max_steps.is_some_and(|n| self.workload_actions >= n)
    }
    fn resources_drained(&self) -> bool {
        self.play.drained()
            && self.typed.drained()
            && self.resources.drained()
            && self.held_buttons.is_empty()
            && self.held_keys.is_empty()
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
            && self
                .hangs
                .max(self.watchdog.count())
                .max(self.output_health.stalls())
                == 0
            && self.boundary_failures == 0
            && !self.cleanup_incomplete
            && self.resources_drained()
            && self.replay.as_ref().is_none_or(VecDeque::is_empty)
            && self.final_diagnostics
            && self.diagnostics_truncated == 0
            && !self.diagnostic_bus_loss
            && !self.report_io_failed
            && self.journal_error.is_none()
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
            && self.host_seed == Some(self.seed)
            && self.native_events > 0
            && self.shell_catalog.complete()
            && self.typed.complete()
    }

    /// Count events actually offered to route_event, rather than generated events
    /// that an earlier rejection prevented from reaching the production owner.
    pub fn record_native_events(&mut self, events: u64) {
        self.native_events += events;
    }

    /// Surface queries precede step_with; attach their elapsed time after its reset.
    pub fn record_surface_setup(&mut self, elapsed: Duration) {
        self.measurements
            .record(measurements::Stage::SurfaceSetup, elapsed);
    }

    /// End-of-budget verification after shell polling catches jobs launched by a
    /// ready modal or build callback later in the frame. Normal frames do no work.
    pub fn observe_shell_resources(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
    ) -> ara_ui::Result<()> {
        if !self.panics.is_empty() {
            return Ok(());
        }
        self.shell_catalog.observe(&editor.extensions);
        if !self.budget_reached() && !self.replay.as_ref().is_some_and(VecDeque::is_empty) {
            return Ok(());
        }
        if self.is_soak() {
            self.shell_catalog.begin_drain();
        }
        self.resources.shell(background_work(editor, shell));
        self.play.poll(editor)?;
        self.refresh_end_inventory(editor)
    }

    fn refresh_end_inventory(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let inventory = editor.action("jobs.list", json!({}))?;
        self.resources.observe(&inventory)?;
        self.typed.observe_jobs(&inventory);
        self.resources.settle_files(editor);
        let catalog = editor.action("schema.list", json!({"kind":"command"}))?;
        self.typed.validate_final_catalog(&catalog)?;
        self.shell_catalog.observe(&editor.extensions);
        let shell_catalog = editor.action("editor.commands", json!({}))?;
        let settled = self.resources_drained();
        self.shell_catalog.reconcile_final(&shell_catalog, settled);
        self.write_shell_catalog_events()?;
        let matched = self.typed.final_catalog_matched();
        if self.resources.take_changed() || self.last_catalog_matched != Some(matched) {
            self.write_event(json!({"phase":"jobInventory","when":"finish","step":self.actions,"resources":self.resources.report(),
                "typedCommandCatalogFinalMatched":matched,
                "shellCommandCatalogFinalMatched":self.shell_catalog.final_matched()}))?;
            self.last_catalog_matched = Some(matched);
        }
        Ok(())
    }

    fn write_shell_catalog_events(&mut self) -> ara_ui::Result<()> {
        for mut event in self.shell_catalog.take_events() {
            event["step"] = self.actions.into();
            self.write_event(event)?;
        }
        Ok(())
    }

    /// Persist the final summary even when an enclosing frame limit ended the run.
    pub fn finish(&mut self) -> ara_ui::Result<()> {
        self.watchdog.finish();
        let journal = self.journal.finish();
        if let Err(error) = &journal {
            self.fail_journal(error.to_string());
        }
        let prior_writes = self.reports.drain_checkpoints();
        self.report_io_failed |= prior_writes.is_err();
        let text = self.report_text(true)?;
        let result = self.reports.finish(text);
        self.report_io_failed |= result.is_err();
        journal?;
        prior_writes?;
        result?;
        Ok(())
    }

    /// Drain the last diagnostics tail even if the duration elapsed between
    /// actions, then write the authoritative final report.
    pub fn finish_with(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.watchdog.progress(self.actions, None);
        if self.resources.initialized() && self.panics.is_empty() {
            self.refresh_end_inventory(editor)?;
        }
        self.read_diagnostics(editor)?;
        self.final_diagnostics = true;
        self.finish()
    }

    fn write_event(&mut self, event: Value) -> ara_ui::Result<()> {
        // JSONL frames use the same compact serializer as the host journal;
        // standalone report documents use Ara's canonical document writer.
        let line = self.journal_line(event)?;
        let at = Instant::now();
        let result = self.journal.record(line);
        self.measurements
            .record(measurements::Stage::JournalSubmit, at.elapsed());
        if let Err(error) = result {
            self.fail_journal(error.to_string());
        }
        Ok(())
    }

    fn journal_line(&mut self, event: Value) -> ara_ui::Result<String> {
        let at = Instant::now();
        let result = serde_json::to_string(&event);
        self.measurements
            .record(measurements::Stage::JournalSerialize, at.elapsed());
        Ok(result?)
    }

    fn record_journal_wait(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.;
        self.journal_wait_ms += ms;
        self.max_journal_wait_ms = self.max_journal_wait_ms.max(ms);
    }

    fn abort_pending(&mut self, reason: &str) -> ara_ui::Result<()> {
        if let Some(pending) = self.pending_action.take() {
            self.record_journal_wait(pending.since.elapsed());
            self.aborted_action = Some(
                json!({"action":pending.json,"ticket":pending.ticket,"step":self.actions,"wasDispatched":false,"reason":reason}),
            );
            self.write_event(json!({"phase":"abort","step":self.actions,"ticket":pending.ticket,"wasDispatched":false,"reason":reason}))?;
        }
        Ok(())
    }

    fn fail_journal(&mut self, reason: String) {
        if let Err(error) = self.abort_pending(&reason) {
            self.journal.fail(error.to_string());
        }
        if self.journal.report()["error"].is_null() {
            self.journal.fail(reason.clone());
        }
        self.journal_error.get_or_insert(reason);
        self.stopped = true;
    }

    fn read_diagnostics(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let at = Instant::now();
        let logs = editor.action(
            "logs",
            json!({"since":self.log_cursor,"severity":"warning","group":false,"limit":1000}),
        );
        self.measurements
            .record(measurements::Stage::DiagnosticsRead, at.elapsed());
        let logs = logs?;
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

    fn write_report(&mut self) -> ara_ui::Result<()> {
        let text = self.report_text(false)?;
        let at = Instant::now();
        let result = self.reports.checkpoint(text);
        self.measurements
            .record(measurements::Stage::ReportSubmit, at.elapsed());
        self.report_io_failed |= result.is_err();
        result?;
        Ok(())
    }

    fn report_text(&mut self, final_snapshot: bool) -> ara_ui::Result<String> {
        let snapshot_at = Instant::now();
        let mut journal = self.journal.report();
        if let Some(error) = journal["error"].as_str() {
            self.fail_journal(error.to_owned());
        }
        journal["pendingAction"] = self.pending_action.as_ref().map_or(Value::Null,|pending|json!({"action":pending.json,
            "ticket":pending.ticket,"step":self.actions,"waitMs":pending.since.elapsed().as_secs_f64()*1000.,"wasDispatched":false}));
        journal["abortedAction"] = self.aborted_action.clone().unwrap_or(Value::Null);
        journal["waitFrames"] = self.journal_wait_frames.into();
        journal["completedWaitMs"] = self.journal_wait_ms.into();
        journal["maxCompletedWaitMs"] = self.max_journal_wait_ms.into();
        journal["ackDeadlineSeconds"] = JOURNAL_ACK.as_secs().into();
        journal["waitScope"] = "Elapsed wall time with an exact staged action withheld; the native UI/Host loop keeps pumping. It is not main-thread blocking time.".into();
        let mut report_writer = self.reports.report();
        self.report_io_failed |= !report_writer["error"].is_null();
        report_writer["finalSnapshotRequested"] = final_snapshot.into();
        report_writer["heartbeatScope"] = "Native input loop and final authority/diagnostic/journal collection; final report storage is awaited after the heartbeat stops".into();
        let unvisited: Vec<_> = self
            .shell_catalog
            .ids()
            .into_iter()
            .filter(|id| !self.commands.contains_key(*id))
            .collect();
        let missing_cases = self.missing_command_cases();
        let accepted = if self.is_soak() {
            self.soak_succeeded()
        } else {
            self.succeeded()
        };
        let mut report = serde_json::Map::new();
        for section in [
            json!({
                "seed":self.seed,"finished":self.finished(),"accepted":accepted,
                "hostSeed":self.host_seed,"hostSeedProvenance":if self.host_seed.is_some() {"startup"} else {"unknown"},
                "replayOriginalHostSeed":self.replay_original_host_seed,
                "replayOriginalHostSeedProvenance":if self.replay_original_host_seed.is_some() {"journal"} else {"unknown"},
                "hostSeedMatchesReplay":self.replay.as_ref().and(self.host_seed.zip(self.replay_original_host_seed).map(|(actual,original)|actual==original)),
                "seconds":self.started.elapsed().as_secs_f64(),"requestedSeconds":self.budget.as_secs_f64(),
                "soakRequested":self.is_soak(),"soakAccepted":self.soak_succeeded(),
                "durationCompleted":self.started.elapsed()>=self.budget,
                "termination":self.termination(),
                "actions":self.actions,"frames":self.frames,"rejectedInputs":self.rejected,
                "offscreenGpuOutput":self.offscreen_output,"gpuSurfaceSubmissions":self.gpu_surface_submissions,
                "nativePresentations":self.native_presentations,
                "requestedSteps":self.max_steps,"workloadActions":self.workload_actions,"cleanupActions":self.cleanup_actions,
                "cleanupIncomplete":self.cleanup_incomplete||!self.resources_drained(),
                "endDrain":{"actions":self.drain_actions,"limitSeconds":DRAIN.as_secs(),"seconds":self.draining_since.map(|since|since.elapsed().as_secs_f64()),
                    "cleanupIncomplete":self.cleanup_incomplete||!self.resources_drained(),"heldKeys":self.held_keys,"heldButtons":self.held_buttons},
            }),
            json!({
                "panics":self.panics,"hangs":self.hangs.max(self.watchdog.count()).max(self.output_health.stalls()),"hangThresholdMs":HANG.as_millis(),
                "longFrames":self.hangs,"watchdogStalls":self.watchdog.count(),
                "gpuOutputStalls":self.output_health.stalls(),"gpuOutputHealth":self.output_health.report(),
                "frameSpikes":self.spikes,"spikeThresholdMs":SPIKE.as_millis(),"maxFrameMs":self.max_frame_ms,
                "slowestFrames":self.slowest,"panelFailures":self.boundary_failures,
                "harnessTimings":self.measurements.summary(),
                "reportWriter":report_writer,
                "journalRecorder":journal,
                "errorDiagnosticsBefore":self.baseline_diagnostics,"errorDiagnosticsDuring":self.diagnostics,"registryCommands":self.commands,
                "baselineDiagnosticsCollected":self.baseline_collected,"diagnosticBusLoss":self.diagnostic_bus_loss,
                "finalDiagnosticsCollected":self.final_diagnostics,"diagnosticRecordsOmitted":self.diagnostics_truncated,
            }),
            json!({
                "commandEdgeCases":self.command_cases,"commandCallbacksAccepted":self.command_successes,
                "protocolMethods":self.methods,"unvisitedCommands":unvisited,
                "uiActions":self.ui_actions,
                "coverageComplete":self.shell_catalog.complete() && self.typed.complete(),"missingCommandEdgeCases":missing_cases,
                "edgeCasesPerCommand":actions::edge_case_count(),"nativeEventsRouted":self.native_events,
                "journal":"monkey-events.jsonl",
                "replayedJournal":self.replay.is_some(),
                "replayRemaining":self.replay.as_ref().map(VecDeque::len),
                "playResources":self.play.report(),
                "watchdog":"monkey-watchdog.json",
                "replay":format!("--verify-monkey --monkey-seed {} --monkey-seconds {} --monkey-steps {}",self.seed,self.budget.as_secs(),self.workload_actions.max(1)),
                "exactReplay":"--monkey-replay <original-output>/monkey-events.jsonl --output <new-output>",
                "limits":["Command coverage counts attempted edge targets; disabled or deferred commands can reject before their implementation runs. Accepted callbacks are counted separately.","Protocol method probes include malformed envelopes and stale sessions; a method attempt is not complete implementation coverage.","The watchdog persists permanent stalls independently; an external process timeout is required to end a permanently stalled editor.","Long frames, watchdog stalls and GPU-output starvation intervals are reported separately; hangs is their max-based lower bound because intervals can overlap.","NaN and infinity reach numeric controls as text because JSON cannot represent non-finite numbers."]
            }),
            self.typed.report(),
            self.shell_catalog.report(),
            json!({"hostJobResources":self.resources.report()}),
        ] {
            let Value::Object(fields) = section else {
                return Err("Monkey report section is not an object".into());
            };
            report.extend(fields);
        }
        let report = Value::Object(report);
        self.measurements.record(
            measurements::Stage::ReportSnapshotBuild,
            snapshot_at.elapsed(),
        );
        let at = Instant::now();
        let text = ara_json::to_canonical_string(&report);
        self.measurements
            .record(measurements::Stage::ReportCanonicalSerialize, at.elapsed());
        Ok(text)
    }

    fn missing_command_cases(&self) -> Vec<Value> {
        self.shell_catalog.missing()
    }

    fn termination(&self) -> &'static str {
        if !self.panics.is_empty() {
            "panic"
        } else if self.journal_error.is_some() {
            "journal-error"
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

fn background_work(editor: &mut Editor, shell: &Shell) -> Value {
    let mut report = shell.monkey_background_work();
    let probes = editor.probe_captures.resource_report();
    report["busy"] = json!(report["busy"] == true || probes["retired"] != true);
    report["probeCaptures"] = probes;
    report
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panic with a non-text payload")
}
