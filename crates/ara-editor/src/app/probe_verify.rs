//! Native J7 probe authoring, derived capture receipts and completed debug views.
mod cache;
mod capture;
mod fixture;
mod input;
mod observe;
mod proof;
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use ara_runtime_render::probes::{ProbeRow, ProbeSnapshot};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SCENE: &str = "sc_nativeprobe";
const CUBE: &str = "e_nativeprobecube";
const SUN: &str = "e_nativeprobesun";
const PATIENCE: Duration = Duration::from_secs(120);
const QUERY_CADENCE: Duration = Duration::from_millis(50);
const CASES: [&str; 8] = [
    "probe-native-create-one-undo-redo",
    "probe-native-resolution-select-64-to-128-one-undo-redo",
    "probe-influence-projection-capture-cages",
    "probe-native-recapture-six-face-published-receipt",
    "probe-native-cancel-active-generation-retains-previous",
    "probe-native-source-edit-rejects-stale-generation",
    "native-completed-gi-diffuse-debug-view",
    "native-completed-overdraw-debug-view",
];

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}
pub(super) struct ProbeCheck {
    output: PathBuf,
    original: Value,
    index: usize,
    phase: u32,
    started: Instant,
    case_started: Instant,
    probe: Option<String>,
    clipboard: Clip,
    pointer: Option<[f32; 2]>,
    before: Value,
    committed: Value,
    observations: Value,
    complete: Option<Value>,
    pending_capture: Option<&'static str>,
    job_baseline: BTreeSet<String>,
    job: Option<String>,
    job_status: Value,
    snapshot: Option<ProbeSnapshot>,
    next_query: Instant,
    previous_receipt: Value,
    active_generation: Option<u64>,
    renderer: Value,
    cache_publication: Option<cache::Publication>,
    view_observation_serial: u64,
    trace: Vec<Value>,
    results: Vec<Value>,
    native_events: usize,
    case_events: usize,
    restored: bool,
    error: Option<String>,
}
impl ProbeCheck {
    pub fn new(editor: &mut Editor, shell: &mut Shell, output: &Path) -> ara_ui::Result<Self> {
        if !shell.offscreen || !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Probe proof requires the isolated offscreen behaviour fixture".into());
        }
        if shell.modal_active() || editor.gesture.is_some() || editor.play_world.is_some() {
            return Err(
                "Probe proof requires no active modal, authoring gesture or Play world".into(),
            );
        }
        std::fs::create_dir_all(output)?;
        let original = fixture::create(editor)?;
        let check = Self {
            output: output.into(),
            original,
            index: 0,
            phase: 0,
            started: Instant::now(),
            case_started: Instant::now(),
            probe: None,
            clipboard: Clip::default(),
            pointer: None,
            before: Value::Null,
            committed: Value::Null,
            observations: json!({}),
            complete: None,
            pending_capture: None,
            job_baseline: BTreeSet::new(),
            job: None,
            job_status: Value::Null,
            snapshot: None,
            next_query: Instant::now(),
            previous_receipt: Value::Null,
            active_generation: None,
            renderer: Value::Null,
            cache_publication: None,
            view_observation_serial: 0,
            trace: Vec::new(),
            results: Vec::new(),
            native_events: 0,
            case_events: 0,
            restored: false,
            error: None,
        };
        check.write()?;
        Ok(check)
    }
    pub fn finished(&self) -> bool {
        self.error.is_some() || (self.index == CASES.len() && self.restored)
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|row| row["ok"] != true)
            .filter_map(|row| row["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }
    /// Request one completed-frame observation after the native mode command.
    /// The caller separately checks the actual retained raster mode and readiness.
    pub fn view_mode_observation_needed(&self) -> Option<&'static str> {
        if self.finished() || self.phase < 4 || self.complete.is_some() || !self.renderer.is_null()
        {
            return None;
        }
        match self.index {
            6 => Some("gi_diffuse"),
            7 => Some("overdraw"),
            _ => None,
        }
    }
    /// `{mode,completed,current,overdraw}` from capture_completed on a current,
    /// ready image whose actual retained raster mode matches `mode`. Request
    /// metadata alone cannot establish this; missing evidence cannot pass.
    pub fn observe_view_mode(&mut self, mut value: Value) -> ara_ui::Result<()> {
        if value["completed"] != true || value["current"] != true {
            return Err("Native debug observation was not a current completed image".into());
        }
        self.view_observation_serial = self
            .view_observation_serial
            .checked_add(1)
            .ok_or("Native debug observation serial exhausted")?;
        value["observationSerial"] = self.view_observation_serial.into();
        value["generationScope"] =
            "completed-image observation serial; not a renderer frame ID".into();
        self.renderer = value;
        Ok(())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "native owners and actual Scene readiness"
    )]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if let Some(pointer) = self.pointer {
            shell.desktop_pointer = Some(pointer);
        }
        let began = events.len();
        let result = self.input(editor, shell, window, scale, events, scene_ready);
        let count = events.len().saturating_sub(began);
        self.native_events += count;
        self.case_events += count;
        if let Err(error) = &result {
            events.truncate(began);
            native::focus_lost(window, events)?;
            shell.desktop_pointer = None;
            self.pointer = None;
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn row(&self) -> Option<&ProbeRow> {
        let entity = self.probe.as_deref()?;
        self.snapshot
            .as_ref()?
            .probes
            .iter()
            .find(|row| row.doc == SCENE && row.entity == entity)
    }
    fn fail(&mut self, editor: &Editor, shell: &Shell, error: &str) -> ara_ui::Result<()> {
        self.error = Some(error.into());
        self.results.push(json!({"journey":CASES.get(self.index),"ok":false,"phase":self.phase,
            "error":error,"before":self.before,"observed":self.evidence(editor,shell),"nativeEvents":self.case_events,"trace":self.trace}));
        self.write()
    }
    pub fn validate(&self) -> ara_ui::Result<()> {
        if self.index != CASES.len()
            || !self.restored
            || self.error.is_some()
            || !self.failures().is_empty()
        {
            return Err(format!(
                "Native probe/debug-view proof incomplete: {:?}; {:?}",
                self.failures(),
                self.error
            )
            .into());
        }
        Ok(())
    }
}
