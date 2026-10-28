//! Native hierarchy acceptance through SDL and the production controls on an isolated project.
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
mod filters;
mod fixture;
mod flags;
mod groups;
mod input;
mod proof;
mod stress;
pub(super) use fixture::prepare;

const SCENE: &str = "sc_hierarchyproof";
const PREFAB: &str = "pf_hierarchyproof";
const STRESS: &str = "sc_hierarchystress";
const COUNT: usize = 100_000;
const COMMON: &str = "e_hcommon";
const LEFT: &str = "e_hleft";
const RIGHT: &str = "e_hright";
const FIRST: &str = "e_hfirst";
const CHILD: &str = "e_hchild";
const LINKED: &str = "e_hlinked";
const OTHER: &str = "e_hother";
const QUERY_ROOT: &str = "e_hqueryroot";
const QUERY_BRANCH: &str = "e_hquerybranch";
const QUERY_LEAF: &str = "e_hqueryleaf";
const QUERY: &str = "type:Light name:\"Key light\" -id:e_hexcluded";
const SAVED: &str = "Native Lights مصابيح";
const PATIENCE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Group,
    Context,
    Flags,
    Query,
    Saved,
    Stress,
}
const ACTS: &[Act] = &[
    Act::Group,
    Act::Context,
    Act::Flags,
    Act::Query,
    Act::Saved,
    Act::Stress,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Group => "group-cross-parent-linked-one-undo-redo",
            Self::Context => "context-group-create-child-f2",
            Self::Flags => "selected-versus-branch-hide-lock",
            Self::Query => "compound-query-ancestor-context-filtered-reveal",
            Self::Saved => "saved-query-prompt-select-delete",
            Self::Stress => "100k-native-range-anchor-escape",
        }
    }
}

#[derive(Default)]
struct Baseline {
    revision: u64,
    records: Value,
    world: BTreeMap<String, [f32; 16]>,
    camera: crate::state::Camera,
    selection: Vec<String>,
    collapsed: BTreeSet<String>,
    hidden: BTreeSet<String>,
    locked: BTreeSet<String>,
}

pub(super) struct HierarchyCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    cooldown: u32,
    started: Instant,
    pending_capture: Option<&'static str>,
    outcome: Option<Value>,
    before: Baseline,
    original_doc: String,
    original_camera: crate::state::Camera,
    original_selection: Vec<String>,
    original_search: String,
    original_focus: String,
    original_filters: Value,
    original_keymap: Value,
    linked_inner: String,
    group: Option<String>,
    child: Option<String>,
    committed: Option<Value>,
    native_events: usize,
    trace: Vec<Value>,
    results: Vec<Value>,
}

impl HierarchyCheck {
    /// Create only typed scratch fixtures; measured actions enter through native SDL events.
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Hierarchy acceptance requires the isolated behaviour fixture".into());
        }
        if editor.host.store().workspace().find(STRESS).is_none() {
            return Err(
                "Prepare the canonical 100k hierarchy fixture before opening the editor".into(),
            );
        }
        std::fs::create_dir_all(output)?;
        let original_doc = editor.state.doc.clone();
        let original_camera = editor.state.camera;
        let original_selection = editor.state.selection.clone();
        let original_search = editor
            .state
            .searches
            .get("hierarchy")
            .cloned()
            .unwrap_or_default();
        let original_focus = editor.state.focus.clone();
        let original_filters = editor
            .state
            .settings
            .get("hierarchy.filters")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let original_keymap = editor
            .state
            .settings
            .get("input.keymap")
            .cloned()
            .unwrap_or_else(|| json!("ara"));
        if original_filters.get(SAVED).is_some() {
            return Err("Hierarchy acceptance refuses a stale saved-filter fixture".into());
        }
        let linked_inner = fixture::create(editor)?;
        editor.action("editor.scene.open", json!({"scene":original_doc}))?;
        editor.action("editor.camera", serde_json::to_value(original_camera)?)?;
        editor.action("editor.select", json!({"entities":original_selection}))?;
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            cooldown: 0,
            started: Instant::now(),
            pending_capture: None,
            outcome: None,
            before: Baseline::default(),
            original_doc,
            original_camera,
            original_selection,
            original_search,
            original_focus,
            original_filters,
            original_keymap,
            linked_inner,
            group: None,
            child: None,
            committed: None,
            native_events: 0,
            trace: Vec::new(),
            results: Vec::new(),
        })
    }

    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
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
        if self.finished() || self.outcome.is_some() {
            return Ok(());
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return Ok(());
        }
        if self.pending_capture.is_some() {
            return Ok(());
        }
        let before = events.len();
        let result = (|| {
            if !scale.is_finite() || scale <= 0. {
                return Err("Hierarchy native scale is invalid".into());
            }
            if self.started.elapsed() >= PATIENCE {
                return Err(format!(
                    "Hierarchy {} phase {} timed out after {} seconds",
                    ACTS[self.index].label(),
                    self.phase,
                    PATIENCE.as_secs()
                )
                .into());
            }
            if self.phase == 0 {
                if self.index == 0 {
                    // Constructors can run well before this route enters the event loop.
                    // Restore the context at actual ingress, not an earlier constructor's scene.
                    self.original_doc = editor.state.doc.clone();
                    self.original_camera = editor.state.camera;
                    self.original_selection = editor.state.selection.clone();
                    self.original_search = editor
                        .state
                        .searches
                        .get("hierarchy")
                        .cloned()
                        .unwrap_or_default();
                    self.original_focus = editor.state.focus.clone();
                    self.original_filters = editor
                        .state
                        .settings
                        .get("hierarchy.filters")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    self.original_keymap = editor
                        .state
                        .settings
                        .get("input.keymap")
                        .cloned()
                        .unwrap_or_else(|| json!("ara"));
                    if self.original_filters.get(SAVED).is_some() {
                        return Err("Hierarchy route found a stale saved-filter fixture".into());
                    }
                }
                fixture::setup(self, editor, shell)?;
                native::focus_lost(window, events)?;
                self.mark(
                    "before",
                    editor,
                    shell,
                    json!({"fixture":"typed setup; native measured gestures"}),
                );
                self.next();
                self.cooldown = 4;
                return Ok(());
            }
            match ACTS[self.index] {
                Act::Group => self.groups(editor, shell, window, scale, events),
                Act::Context => self.context(editor, shell, window, scale, events),
                Act::Flags => self.flags(editor, shell, window, scale, events),
                Act::Query => self.query(editor, shell, window, scale, events),
                Act::Saved => self.saved(editor, shell, window, scale, events),
                Act::Stress => self.stress(editor, shell, window, scale, events),
            }
        })();
        self.native_events += events.len().saturating_sub(before);
        if let Err(error) = result {
            events.truncate(before);
            native::tap(window, 41, events)?;
            native::focus_lost(window, events)?;
            self.outcome = Some(json!({"ok":false,"error":error.to_string(),"phase":self.phase}));
            self.pending_capture = Some("failure");
        }
        Ok(())
    }

    fn next(&mut self) {
        self.phase += 1;
        self.cooldown = 2;
        self.started = Instant::now();
    }
    fn mark(&mut self, stage: &'static str, editor: &Editor, shell: &Shell, detail: Value) {
        self.pending_capture = Some(stage);
        self.trace.push(json!({"stage":stage,"phase":self.phase,"revision":editor.host.store().revision(),
            "selection":proof::selection(editor),"focus":proof::focus(shell),"hierarchyOffset":shell.hierarchy_offset,
            "detail":detail}));
    }
    fn pass(&mut self) {
        self.outcome = Some(json!({"ok":true}));
        self.pending_capture = Some("after");
    }

    /// Capture only the completed main native target; compact traces never expand 100k records.
    pub fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        stage: &str,
    ) -> ara_ui::Result<()> {
        let mut prefix = format!(
            "hierarchy-{:02}-{}-{stage}",
            self.index + 1,
            ACTS[self.index].label()
        );
        if !matches!(stage, "before" | "interaction" | "after" | "failure") {
            prefix.push_str(&format!("-phase{}", self.phase));
        }
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{prefix}.png")))?;
        std::fs::write(
            self.output.join(format!("{prefix}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        self.trace.push(json!({"capture":stage,"prefix":prefix,"doc":editor.state.doc,"selection":proof::selection(editor),
            "rows":proof::rows(shell),"focus":proof::focus(shell),"revision":editor.host.store().revision(),
            "modal":shell.modal_active(),"choice":shell.choice_open(),"captureHeld":shell.input.captured()}));
        Ok(())
    }

    /// Called once after native rendering; a finished temporary scene is left through typed navigation.
    pub fn end_case(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if self.cooldown > 0 {
            return Ok(());
        }
        if let Some(stage) = self.pending_capture.take() {
            self.capture(editor, shell, gpu, stage)?;
        }
        let Some(outcome) = self.outcome.take() else {
            return Ok(());
        };
        self.results.push(json!({"journey":ACTS[self.index].label(),"ok":outcome["ok"],"outcome":outcome,
            "beforeRevision":self.before.revision,"afterRevision":editor.host.store().revision(),"trace":self.trace}));
        if ACTS[self.index] == Act::Stress {
            editor.action("editor.scene.open", json!({"scene":SCENE}))?;
            proof::require(
                editor.state.doc == SCENE,
                "The temporary 100k story did not restore the previous scene",
            )?;
            if let Some(result) = self.results.last_mut() {
                result["temporarySceneRestored"] = true.into();
            }
        }
        self.index += 1;
        self.phase = 0;
        self.cooldown = 4;
        self.started = Instant::now();
        self.trace.clear();
        self.group = None;
        self.child = None;
        self.committed = None;
        if self.finished() {
            editor.action(
                "editor.settings.set",
                json!({"key":"hierarchy.filters","value":self.original_filters}),
            )?;
            editor.action(
                "editor.settings.set",
                json!({"key":"input.keymap","value":self.original_keymap}),
            )?;
            editor.action("editor.scene.open", json!({"scene":self.original_doc}))?;
            editor.action("editor.camera", serde_json::to_value(self.original_camera)?)?;
            editor.action("editor.select", json!({"entities":self.original_selection}))?;
            editor.action(
                "editor.search",
                json!({"panel":"hierarchy","query":self.original_search}),
            )?;
            editor.action("editor.focus", json!({"panel":self.original_focus}))?;
            shell.rebuild(editor)?;
            proof::require(
                editor.state.doc == self.original_doc
                    && editor.state.camera == self.original_camera
                    && editor.state.selection == self.original_selection,
                "Hierarchy acceptance did not restore its caller's navigation context",
            )?;
        }
        self.write()
    }

    pub fn report(&self) -> Value {
        json!({"route":"SDL_PushEvent -> Window::poll_tagged -> production hierarchy/palette/context/dialog controls",
            "completed":self.finished(),"passed":self.results.iter().filter(|row|row["ok"]==true).count(),
            "failed":self.failures().len(),"nativeEvents":self.native_events,"journeys":self.results,
            "savedFilterReopen":"covered by isolated-profile hierarchy_filters unit test; native route tests save/select/delete",
            "stress":{"entities":COUNT,"geometry":false,"trace":"compact counts, endpoints and hashes"}})
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("hierarchy.json"),
            ara_json::to_canonical_string(&self.report()),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara native hierarchy acceptance</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}img{max-width:47%;margin:8px}</style><h1>Native hierarchy acceptance</h1><p><a href='hierarchy.json'>Structured outcomes and native input trace</a></p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!("<section><h2>{}</h2>", act.label()));
            for stage in ["before", "interaction", "after"] {
                let file = format!("hierarchy-{:02}-{}-{stage}.png", index + 1, act.label());
                html.push_str(&format!(
                    "<a href='{file}'><img src='{file}' alt='{stage}'></a>"
                ));
            }
            html.push_str("</section>");
        }
        std::fs::write(self.output.join("hierarchy-index.html"), html)?;
        Ok(())
    }
}
