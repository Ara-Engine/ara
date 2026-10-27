//! Mounted panel failure/retry proof through the production guard and native input loop.
use super::*;
use crate::shell::ProbeFailure;
mod input;
mod proof;

const A: &str = "e_boundarya1";
const B: &str = "e_boundaryb1";
const FINISH: u32 = 22;
#[derive(Clone, Copy)]
struct Case {
    panel: &'static str,
    kind: ProbeFailure,
    label: &'static str,
}
const CASES: &[Case] = &[
    Case {
        panel: "inspector",
        kind: ProbeFailure::Error,
        label: "inspector-returned-error",
    },
    Case {
        panel: "assets",
        kind: ProbeFailure::Panic,
        label: "assets-caught-panic",
    },
];

pub(super) struct BoundaryCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    revision: u64,
    before: Value,
    correlation: String,
    diagnostics: Vec<Value>,
    trace: Vec<Value>,
    results: Vec<Value>,
}
impl BoundaryCheck {
    pub fn new(editor: &mut Editor, output: &std::path::Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err(
                "Panel boundary verification requires its isolated behaviour fixture".into(),
            );
        }
        editor.apply(vec![
            json!({"cmd":"entity.create","args":{"id":A,"name":"Boundary recovery A","components":{"Transform":{},"MeshRenderer":{}}}}),
            json!({"cmd":"entity.create","args":{"id":B,"name":"Boundary recovery B","components":{"Transform":{"position":[2,0,0]},"MeshRenderer":{}}}}),
        ],"Create panel recovery selection fixtures",None)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            revision: 0,
            before: Value::Null,
            correlation: String::new(),
            diagnostics: Vec::new(),
            trace: Vec::new(),
            results: Vec::new(),
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= CASES.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
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
        let Some(&case) = CASES.get(self.index) else {
            return Ok(());
        };
        if self.age == 0 {
            return self.setup(case, editor, shell);
        }
        if matches!(self.age, 5 | 17 | FINISH) {
            self.read_diagnostics(editor)?;
        }
        self.input(case, editor, shell, window, scale, events)
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        let Some(&case) = CASES.get(self.index) else {
            return Ok(());
        };
        if let Some(stage) = match self.age {
            2 => Some("before"),
            5 => Some("error"),
            9 => Some("healthy-input"),
            12 => Some("menu"),
            17 => Some("recovered"),
            _ => None,
        } {
            self.capture(case, stage, editor, shell, gpu)?;
        }
        if self.age >= FINISH {
            let (ok, detail) = self.result(case, editor, shell)?;
            self.results.push(json!({"journey":case.label,"ok":ok,"correlation":self.correlation,"diagnostics":self.diagnostics,"detail":detail,"trace":self.trace}));
            shell.disarm_panel_probe();
            self.index += 1;
            self.age = 0;
            self.trace.clear();
            self.diagnostics.clear();
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("panel-boundaries.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results,
                "scope":"Real mounted panel render, production catch/staging/quarantine, raw SDL healthy controls and Retry; hidden native GPU target",
                "expectedFaults":"One intentional returned error and one caught panic, each with exactly one correlated ARA4500. These are verification faults, not soak panic counts.",
                "soakIsolation":"The hook is one-shot, fixture/offscreen-only, unregistered, disarmed after each case. Long monkey runs start fresh processes."
            })),
        )?;
        Ok(())
    }
}

fn records(editor: &Editor) -> ara_ui::Result<Value> {
    Ok(json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("Boundary proof scene missing")?
            .records()
    ))
}

impl BoundaryCheck {
    fn setup(&mut self, case: Case, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if !shell.offscreen || shell.panel_failure_count() != 0 || shell.panel_probe_armed() {
            return Err("Recovery proof requires an offscreen shell without a pre-existing panel failure or armed probe".into());
        }
        shell.leave();
        shell.menu = None;
        shell.desktop_pointer = None;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action(
            "editor.layout.collapse",
            json!({"area":"bottom","collapsed":false}),
        )?;
        editor.action("editor.layout.show", json!({"panel":case.panel}))?;
        editor.action("editor.select", json!({"entities":[A]}))?;
        editor.action(
            "editor.search",
            json!({"panel":"hierarchy","query":"Boundary recovery"}),
        )?;
        editor.action("editor.search", json!({"panel":"assets","query":""}))?;
        shell.rebuild(editor)?;
        self.revision = editor.host.store().revision();
        self.before = records(editor)?;
        self.correlation = format!("P4-boundary-{}-{}", case.label, self.index + 1);
        Ok(())
    }
    fn read_diagnostics(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let logs = editor.action(
            "logs",
            json!({"codes":["ARA4500"],"group":false,"limit":1000}),
        )?;
        self.diagnostics = logs["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| {
                entry["diagnostic"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(&self.correlation))
            })
            .cloned()
            .collect();
        Ok(())
    }
}
