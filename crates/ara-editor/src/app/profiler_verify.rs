//! Native Profiler ownership proof over the real Play mailbox; no substitute backend.
mod fixture;
pub(super) use fixture::prepare;
mod input;
mod proof;
#[cfg(test)]
mod tests;
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const PATIENCE: Duration = Duration::from_secs(120);
const CASES: [&str; 8] = [
    "native-play-and-profiler-roster",
    "native-select-client-start-recording",
    "native-client-snapshot-retains-recording",
    "native-client-freeze-stops-and-stays-immutable",
    "native-client-scoped-baseline",
    "native-second-client-distinct-capture",
    "native-cross-client-scope-labelled-comparison",
    "native-stop-replacement-retains-stale-target-without-fallback",
];
pub(super) struct ProfilerCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    started: Instant,
    pointer: Option<[f32; 2]>,
    targets: Vec<Value>,
    world: Option<String>,
    before_serial: u64,
    frozen: Value,
    scene: String,
    revision: u64,
    observed: Value,
    observed_at: Instant,
    complete: Option<Value>,
    error: Option<String>,
    results: Vec<Value>,
    trace: Vec<Value>,
    native_events: usize,
    case_events: usize,
    stability: Option<Instant>,
}
impl ProfilerCheck {
    pub fn new(editor: &mut Editor, shell: &mut Shell, output: &Path) -> ara_ui::Result<Self> {
        if !shell.offscreen || !editor.host.root().join(".ara-profiler-fixture").is_file() {
            return Err("Native Profiler proof requires its isolated offscreen fixture".into());
        }
        fixture::setup(editor, shell)?;
        let scene = proof::scene(editor)?;
        let revision = editor.host.store().revision();
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            started: Instant::now(),
            pointer: None,
            targets: Vec::new(),
            world: None,
            before_serial: 0,
            frozen: Value::Null,
            scene,
            revision,
            observed: Value::Null,
            observed_at: Instant::now() - Duration::from_secs(1),
            complete: None,
            error: None,
            results: Vec::new(),
            trace: Vec::new(),
            native_events: 0,
            case_events: 0,
            stability: None,
        })
    }
    pub fn finished(&self) -> bool {
        self.error.is_some() || self.index == CASES.len()
    }
    fn next(&mut self) {
        self.phase += 1;
        self.started = Instant::now();
        self.observed_at = Instant::now() - Duration::from_secs(1)
    }
    fn done(&mut self, value: Value) {
        self.complete = Some(value)
    }
    fn observation(&mut self, shell: &Shell) {
        if self.observed_at.elapsed() >= Duration::from_millis(250) {
            self.observed = shell.profiler_targets_report();
            self.observed_at = Instant::now();
        }
    }
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() || self.complete.is_some() {
            return Ok(());
        }
        if let Some(pointer) = self.pointer {
            shell.desktop_pointer = Some(pointer)
        }
        self.observation(shell);
        let before = events.len();
        let result = self.input(editor, shell, window, scale, events, scene_ready);
        let added = events.len().saturating_sub(before);
        self.native_events += added;
        self.case_events += added;
        if let Err(error) = result {
            self.error = Some(error.to_string());
            self.results.push(json!({"journey":CASES[self.index],"ok":false,"phase":self.phase,"error":error.to_string(),"nativeEvents":self.case_events,"observed":self.observed}));
            self.write()?;
            native::focus_lost(window, events)?;
        }
        Ok(())
    }
    /// Request a fresh main-surface draw only for a pending evidence capture.
    pub fn wants_frame(&self) -> bool {
        !self.finished() && self.complete.is_some()
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        scene_ready: bool,
        main_ui_submitted: bool,
    ) -> ara_ui::Result<()> {
        if !main_ui_submitted
            || self.finished()
            || (!scene_ready && editor.state.maximized.as_deref() != Some("profiler"))
        {
            return Ok(());
        }
        let Some(evidence) = self.complete.take() else {
            return Ok(());
        };
        if proof::scene(editor)? != self.scene || editor.host.store().revision() != self.revision {
            return Err("Profiler controls changed the authored Scene or content history".into());
        }
        let geometry = crate::shell::feedback_geometry::audit(&shell.ui);
        if !geometry.violations.is_empty() {
            return Err(format!(
                "Profiler capture feedback geometry failed: {:?}",
                geometry.violations
            )
            .into());
        }
        gpu.select_target(0);
        let stem = format!("profiler-{:02}-{}", self.index + 1, CASES[self.index]);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        std::fs::write(
            self.output.join(format!("{stem}.feedback.json")),
            ara_json::to_canonical_string(
                &json!({"surfaces":geometry.surfaces,"violations":geometry.violations}),
            ),
        )?;
        self.results.push(json!({"journey":CASES[self.index],"ok":true,"nativeEvents":self.case_events,"evidence":evidence,"actualReport":shell.profiler_targets_report(),"visible":proof::visible(shell),"sceneUnchanged":true,"authoringHistoryUnchanged":true}));
        self.index += 1;
        self.phase = 0;
        self.started = Instant::now();
        self.case_events = 0;
        self.stability = None;
        if self.finished() {
            self.pointer = None;
            shell.desktop_pointer = None;
        }
        self.write()
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("profiler-targets.json"),
            ara_json::to_canonical_string(&json!({
                "schema":"ara.editor.profiler-native/1","completed":self.index==CASES.len(),"passed":self.results.iter().filter(|r|r["ok"]==true).count(),"failed":self.results.iter().filter(|r|r["ok"]!=true).count(),"error":self.error,"journeys":self.results,"nativeEvents":self.native_events,"trace":self.trace,
                "scope":"Actual SDL_PushEvent -> Window::poll_tagged -> production editor controls -> real PlayProfilingHandle; typed authoring/policy setup before measured gestures only",
                "timingLabel":"informal (shared GPU)","timingAcceptance":false,"current":{"journey":CASES.get(self.index),"phase":self.phase,"phaseSeconds":self.started.elapsed().as_secs_f64()}
            })),
        )?;
        Ok(())
    }
    pub fn validate(&self) -> ara_ui::Result<()> {
        if self.index != CASES.len()
            || self.error.is_some()
            || self
                .results
                .iter()
                .any(|r| r["ok"] != true || r["nativeEvents"].as_u64().is_none_or(|n| n == 0))
        {
            return Err(format!("Native Profiler proof incomplete: {:?}", self.error).into());
        }
        Ok(())
    }
}
