//! Real History controls and authority Undo/Redo through the hidden native loop.
use super::*;
use ara_ui_text::Clipboard;
mod input;
mod proof;

const ENTITY: &str = "e_historynative1";
const FINISH: u32 = 22;
#[derive(Default)]
struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.to_owned();
        Ok(())
    }
}

pub(super) struct HistoryCheck {
    output: PathBuf,
    age: u32,
    finished: bool,
    before: Value,
    after: Value,
    txn: String,
    revision: u64,
    camera: crate::state::Camera,
    selection: Vec<String>,
    clipboard: MemoryClipboard,
    trace: Vec<Value>,
    result: Option<Value>,
}
impl HistoryCheck {
    pub fn new(editor: &mut Editor, output: &std::path::Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("History verification requires its isolated behaviour fixture".into());
        }
        Ok(Self {
            output: output.into(),
            age: 0,
            finished: false,
            before: Value::Null,
            after: Value::Null,
            txn: String::new(),
            revision: 0,
            camera: editor.state.camera,
            selection: Vec::new(),
            clipboard: MemoryClipboard::default(),
            trace: Vec::new(),
            result: None,
        })
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }
    pub fn failures(&self) -> Vec<String> {
        self.result
            .iter()
            .filter(|result| result["ok"] != true)
            .map(|_| "history-native-inspect-copy-undo-redo".into())
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
        if self.finished {
            return Ok(());
        }
        if self.age == 0 {
            return self.setup(editor, shell);
        }
        self.input(editor, shell, window, scale, events)
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.finished {
            return Ok(());
        }
        if let Some(stage) = match self.age {
            2 => Some("changed"),
            6 => Some("selected-copy"),
            10 => Some("undo"),
            14 => Some("redo"),
            19 => Some("search"),
            _ => None,
        } {
            self.capture(stage, editor, shell, gpu)?;
        }
        if self.age >= FINISH {
            self.result = Some(self.observe(editor)?);
            self.finished = true;
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("history.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.finished,"passed":usize::from(self.result.as_ref().is_some_and(|result|result["ok"]==true)),"failed":self.failures().len(),"journeys":self.result.iter().collect::<Vec<_>>(),"trace":self.trace,
                "scope":"Real Store transactions and raw SDL row/copy/Undo button/Redo shortcut/search; offscreen native GPU captures; isolated clipboard"
            })),
        )?;
        Ok(())
    }
    fn setup(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if !shell.offscreen || shell.modal_active() {
            return Err(
                "History proof requires an idle offscreen shell without another modal".into(),
            );
        }
        shell.leave();
        shell.menu = None;
        shell.desktop_pointer = None;
        editor.apply(vec![json!({"cmd":"entity.create","args":{"id":ENTITY,"name":"History original","components":{"Transform":{}}}})],"History acceptance create",None)?;
        self.before = records(editor)?;
        editor.apply(vec![json!({"cmd":"property.set","args":{"entity":ENTITY,"path":"name","value":"History changed"}})],"History acceptance rename",None)?;
        self.after = records(editor)?;
        self.txn = editor
            .host
            .store()
            .history()
            .last()
            .ok_or("History fixture produced no transaction")?
            .id
            .0
            .clone();
        self.revision = editor.host.store().revision();
        editor.action(
            "editor.layout.move",
            json!({"panel":"history","slot":"view"}),
        )?;
        editor.action("editor.focus", json!({"panel":"history"}))?;
        editor.action("editor.search", json!({"panel":"history","query":ENTITY}))?;
        editor.action("editor.select", json!({"entities":[ENTITY]}))?;
        self.camera = editor.state.camera;
        self.selection = editor.state.selection.clone();
        shell.rebuild(editor)
    }
}
fn records(editor: &Editor) -> ara_ui::Result<Value> {
    Ok(json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("History proof scene is missing")?
            .records()
    ))
}
