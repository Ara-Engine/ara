//! J10 combines raw SDL authoring controls with the shared real-ECS reload proof.
mod input;
mod observe;
use super::journey::script_reload::{Proof, SPEED, Stage};
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const ENTITY: &str = "e_nativej10";
const PATIENCE: Duration = Duration::from_secs(180);
const CASES: [&str; 8] = [
    "create-rename-compile-attach-native",
    "module-chooser-explicit-open-native",
    "play-toolbar-native",
    "changed-source-real-ecs-9x",
    "broken-source-last-good-ecs",
    "problems-file-line-and-why-native",
    "fixed-source-structured-state-and-history",
    "stop-toolbar-native",
];

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

pub(super) struct ScriptCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    started: Instant,
    doc: String,
    source: Option<PathBuf>,
    module: Option<String>,
    proof: Option<Proof>,
    pending_capture: Option<&'static str>,
    complete: Option<Value>,
    error: Option<String>,
    results: Vec<Value>,
    trace: Vec<Value>,
    native_events: usize,
    case_events: usize,
    clipboard: Clip,
    pointer: Option<[f32; 2]>,
    world: Option<String>,
    fixture: Value,
    fixed_runtime: bool,
}
impl ScriptCheck {
    pub fn new(editor: &mut Editor, shell: &mut Shell, output: &Path) -> ara_ui::Result<Self> {
        if !shell.offscreen || !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Native J10 requires the isolated offscreen behaviour fixture".into());
        }
        if editor.gesture.is_some() || shell.modal_active() || editor.play_world.is_some() {
            return Err("Native J10 requires no preceding gesture, modal or Play world".into());
        }
        shell.leave();
        shell.open_ide = false;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action(
            "editor.workspace",
            json!({"asset":"","folder":ara_doc::AUTHORING_ROOT}),
        )?;
        for panel in ["hierarchy", "problems"] {
            editor.action("editor.search", json!({"panel":panel,"query":""}))?;
        }
        let before = editor.host.store().revision();
        let fixture = editor.apply(
            vec![json!({"cmd":"entity.create","args":{"id":ENTITY,
            "name":"Native J10 Spinner","index":0,"components":{"Transform":{}}}})],
            "Native J10 fixture entity (protocol setup; not a native gesture)",
            None,
        )?;
        for (key, value) in [
            ("play.tickHz", json!("60")),
            ("play.hotReload", json!(true)),
            ("play.startPaused", json!(false)),
            ("play.mode", json!("local")),
            ("scripts.ide", json!("")),
        ] {
            editor.action("editor.settings.set", json!({"key":key,"value":value}))?;
        }
        shell.rebuild(editor)?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            started: Instant::now(),
            doc: editor.state.doc.clone(),
            source: None,
            module: None,
            proof: None,
            pending_capture: None,
            complete: None,
            error: None,
            results: Vec::new(),
            trace: Vec::new(),
            native_events: 0,
            case_events: 0,
            clipboard: Default::default(),
            pointer: None,
            world: None,
            fixed_runtime: false,
            fixture: json!({"ingress":"Protocol fixture setup only","revisionBefore":before,
                "revisionAfter":editor.host.store().revision(),"entityCreation":fixture,"plannedSpeedBeforePlay":SPEED,"tickHz":60}),
        })
    }
    pub fn finished(&self) -> bool {
        self.error.is_some() || self.index == CASES.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if let Some(pointer) = self.pointer {
            shell.desktop_pointer = Some(pointer)
        }
        let count = events.len();
        let result = self.input(editor, shell, window, scale, events);
        let added = events.len() - count;
        self.native_events += added;
        self.case_events += added;
        if let Err(error) = &result {
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn next(&mut self) {
        self.phase += 1;
        self.started = Instant::now();
    }
    fn done(&mut self, result: Value) {
        self.complete = Some(result);
    }
    fn fail(&mut self, editor: &Editor, shell: &Shell, error: &str) -> ara_ui::Result<()> {
        self.error = Some(error.into());
        self.results.push(json!({"journey":CASES.get(self.index).copied().unwrap_or("evidence-write"),"ok":false,"phase":self.phase,"error":error,
            "nativeEvents":self.case_events,"observed":self.snapshot(editor,shell)}));
        self.write()
    }
    pub fn validate(&self) -> ara_ui::Result<()> {
        if self.index != CASES.len() || !self.failures().is_empty() || self.error.is_some() {
            return Err(format!(
                "Native J10 incomplete or failed: {:?}; {:?}",
                self.failures(),
                self.error
            )
            .into());
        }
        Ok(())
    }
}
