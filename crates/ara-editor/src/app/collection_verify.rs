//! Native J4 collection proof: held edge drags and mixed item-100 edits through SDL.
use super::*;
use ara_ui::collection::ItemKey;
use std::path::Path;
mod fixture;
mod input;
mod proof;

const FIRST: &str = "e_collectionnative1";
const SECOND: &str = "e_collectionnative2";
const COUNT: usize = 160;
const LIST: &str = "MeshRenderer.emissiveMaterials";
const KEYED: &str = "Tilemap.layers";
const MAP: &str = "AudioGraphSource.parameters";
const EDIT: &str = "item 100 changed through native input";
const PATIENCE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    ListDrag,
    KeyedDrag,
    ListEdit,
    MapEdit,
}
const ACTS: &[Act] = &[Act::ListDrag, Act::KeyedDrag, Act::ListEdit, Act::MapEdit];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::ListDrag => "list-held-edge-after-item100-one-undo",
            Self::KeyedDrag => "keyed-held-edge-after-item100-per-object-order",
            Self::ListEdit => "mixed-unequal-list-item100-native-edit",
            Self::MapEdit => "mixed-map-item100-native-edit",
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::ListDrag | Self::ListEdit => LIST,
            Self::KeyedDrag => KEYED,
            Self::MapEdit => MAP,
        }
    }
    fn drag(self) -> bool {
        matches!(self, Self::ListDrag | Self::KeyedDrag)
    }
    fn key(self, index: usize) -> ItemKey {
        match self {
            Self::KeyedDrag => ItemKey::Key(format!("l_c{index:04}")),
            Self::MapEdit => ItemKey::Key(format!("k{index:03}")),
            _ => ItemKey::Index(index),
        }
    }
}

pub(super) struct CollectionCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    started: Instant,
    case_started: Instant,
    input_scale: f32,
    waiting: bool,
    stopped: bool,
    restored: bool,
    initial: Value,
    before: Value,
    expected: Value,
    revision: u64,
    trace: Vec<Value>,
    results: Vec<Value>,
    pending_error: Option<String>,
    edge: [f32; 2],
    start_offset: f32,
    held_frames: u64,
    hold_motion_events: u64,
    native_events: u64,
    max_rows: usize,
    edge_captured: bool,
    selection: Vec<String>,
    maximized: Option<String>,
    search: String,
}
impl CollectionCheck {
    /// Author fixtures only in the behaviour route's isolated scratch project.
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Collection proof requires the isolated behaviour fixture".into());
        }
        let selection = editor.state.selection.clone();
        let maximized = editor.state.maximized.clone();
        let search = editor
            .state
            .searches
            .get("inspector")
            .cloned()
            .unwrap_or_default();
        fixture::create(editor)?;
        let initial = fixture::records(editor)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            started: Instant::now(),
            case_started: Instant::now(),
            input_scale: 1.,
            waiting: false,
            stopped: false,
            restored: false,
            initial,
            before: Value::Null,
            expected: Value::Null,
            revision: 0,
            trace: Vec::new(),
            results: Vec::new(),
            pending_error: None,
            edge: [0.; 2],
            start_offset: 0.,
            held_frames: 0,
            hold_motion_events: 0,
            native_events: 0,
            max_rows: 0,
            edge_captured: false,
            selection,
            maximized,
            search,
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len() && self.restored
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|row| row["ok"] != true)
            .filter_map(|row| row["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.stopped || self.finished() {
            return Ok(());
        }
        if self.pending_error.is_some() {
            return Ok(());
        }
        self.waiting = false;
        self.input_scale = scale;
        if self.index >= ACTS.len() {
            editor.action(
                "editor.search",
                json!({"panel":"inspector","query":self.search}),
            )?;
            editor.action(
                "editor.view",
                json!({"maximized":self.maximized.as_deref().unwrap_or("")}),
            )?;
            editor.action("editor.select", json!({"entities":self.selection}))?;
            shell.rebuild(editor)?;
            self.restored = true;
            return self.write();
        }
        if self.started.elapsed() >= PATIENCE {
            self.pending_error = Some(format!(
                "Native collection story timed out at phase {} ({})",
                self.phase,
                ACTS[self.index].label()
            ));
            return Ok(());
        }
        let before = events.len();
        let result = self.input(editor, shell, window, scale, events);
        self.native_events += (events.len() - before) as u64;
        if let Err(error) = result {
            self.pending_error = Some(error.to_string());
        }
        Ok(())
    }
    fn setup(&mut self, act: Act, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if !shell.offscreen || shell.modal_active() {
            return Err("Collection proof requires an idle offscreen shell".into());
        }
        shell.leave();
        shell.menu = None;
        shell.desktop_pointer = None;
        fixture::restore(editor, &self.initial)?;
        let selection = if act == Act::ListDrag {
            vec![FIRST]
        } else {
            vec![FIRST, SECOND]
        };
        editor.action("editor.select", json!({"entities":selection}))?;
        editor.action("editor.view", json!({"maximized":"inspector"}))?;
        editor.action("editor.focus", json!({"panel":"inspector"}))?;
        // Initial panel configuration only. Reaching row 100 and every tested
        // edit/reorder/Undo/Redo below use native pointer, wheel and key events.
        editor.action(
            "editor.search",
            json!({"panel":"inspector","query":act.field()}),
        )?;
        shell.rebuild(editor)?;
        self.before = fixture::records(editor)?;
        self.expected = fixture::expected(act, &self.before)?;
        self.revision = editor.host.store().revision();
        self.trace.clear();
        self.held_frames = 0;
        self.hold_motion_events = 0;
        self.max_rows = 0;
        self.edge_captured = false;
        self.started = Instant::now();
        self.case_started = self.started;
        Ok(())
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.finished() || self.stopped {
            return Ok(());
        }
        let act = ACTS[self.index];
        if let Some(error) = self.pending_error.take() {
            self.capture("failure", editor, shell, gpu)?;
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error,"phase":self.phase,"elapsedSeconds":self.case_started.elapsed().as_secs_f64(),
                "before":fixture::signature(&self.before),"expected":fixture::signature(&self.expected),"trace":self.trace}));
            self.stopped = true;
            self.write()?;
            return Err(format!("Native collection journey failed: {error}").into());
        }
        self.max_rows = self.max_rows.max(proof::pool(shell, act));
        if self.max_rows > 14 {
            self.pending_error = Some(format!(
                "Collection mounted {} rows instead of a bounded viewport pool",
                self.max_rows
            ));
            return Ok(());
        }
        if self.waiting {
            return Ok(());
        }
        let checked = self.checkpoint(act, editor, shell, gpu);
        if let Err(error) = checked {
            self.pending_error = Some(error.to_string());
            return Ok(());
        }
        if self.waiting {
            return Ok(());
        }
        if self.phase == 11 {
            self.results.push(json!({"journey":act.label(),"ok":true,"elapsedSeconds":self.case_started.elapsed().as_secs_f64(),"beforeRevision":self.revision,"afterRevision":editor.host.store().revision(),
                "heldFramesWithoutMotion":self.held_frames,"heldMotionEvents":self.hold_motion_events,"maxMountedRows":self.max_rows,
                "before":fixture::signature(&self.before),"expected":fixture::signature(&self.expected),"trace":self.trace}));
            self.index += 1;
            self.phase = 0;
        } else {
            self.phase += 1;
        }
        self.started = Instant::now();
        self.write()
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("collections.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.finished(),"stoppedOnFailure":self.stopped,"expected":ACTS.len(),"attempted":self.results.len(),
                "passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len(),"nativeEvents":self.native_events,
                "route":"raw SDL -> Window::poll_tagged -> production editor router -> shared collection controls/DragSession -> typed ops",
                "snapshotSchema":"two complete fixture records asserted exactly; repeated evidence uses canonical JSON byte length and BLAKE3",
                "journeys":self.results,"current":{"journey":ACTS.get(self.index).map(|act|act.label()),"phase":self.phase,"phaseSeconds":self.started.elapsed().as_secs_f64(),"maxRows":self.max_rows}
            })),
        )?;
        Ok(())
    }
}
