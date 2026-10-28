//! Native J5 prefab actions: inspector/menu input, atomic undo and source provenance.
use super::*;
use std::path::Path;
mod fixture;
mod input;
mod observe;

const SCENE: &str = "sc_j5proof";
const BASE: &str = "pf_j5base";
const OUTER: &str = "pf_j5outer";
const VARIANT: &str = "pf_j5variant";
const FIRST: &str = "e_j5first";
const SECOND: &str = "e_j5second";
const VARIANT_ENTITY: &str = "e_j5variant";
const FAILURE: &str = "e_j5failure";
const BEFORE: u32 = 4;
const MENU: u32 = 11;
const COMMITTED: u32 = 18;
const AFTER: u32 = 26;
const PATIENCE: u32 = 144 * 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    ApplyUndo,
    NestedNearest,
    NestedExplicit,
    VariantCreate,
    VariantApply,
    FailedApply,
}
const ACTS: &[Act] = &[
    Act::ApplyUndo,
    Act::NestedNearest,
    Act::NestedExplicit,
    Act::VariantCreate,
    Act::VariantApply,
    Act::FailedApply,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::ApplyUndo => "source-instance-one-undo",
            Self::NestedNearest => "nested-nearest-source",
            Self::NestedExplicit => "nested-explicit-base-source",
            Self::VariantCreate => "native-linked-variant-create",
            Self::VariantApply => "variant-apply-preserves-base",
            Self::FailedApply => "invalid-scene-reference-atomic-failure",
        }
    }
}

pub(super) struct PrefabCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waited: u32,
    waiting: bool,
    failed_frame: bool,
    base_root: String,
    base_child: String,
    nested: String,
    before: Value,
    before_revision: u64,
    before_history: usize,
    committed: Option<Value>,
    log_cursor: u64,
    diagnostics: Value,
    trace: Vec<Value>,
    results: Vec<Value>,
}
impl PrefabCheck {
    /// Author only the isolated behaviour fixture; native input performs each tested action.
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Prefab verification requires the isolated behaviour fixture".into());
        }
        std::fs::create_dir_all(output)?;
        let (base_root, base_child, nested) = fixture::create(editor)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waited: 0,
            waiting: false,
            failed_frame: false,
            base_root,
            base_child,
            nested,
            before: Value::Null,
            before_revision: 0,
            before_history: 0,
            committed: None,
            log_cursor: 0,
            diagnostics: Value::Null,
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
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if let Err(error) = self.input(act, editor, shell, window, scale, events) {
            shell.leave();
            shell.desktop_pointer = None;
            events.clear();
            events.push((window.id(), Event::FocusLost));
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    /// Read the completed hidden native target after production input/rendering.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed_frame) || self.finished() {
            return Ok(());
        }
        if self.waiting {
            self.waited += 1;
            return Ok(());
        }
        let act = ACTS[self.index];
        if let Some(stage) = match self.age {
            BEFORE => Some("before"),
            MENU if matches!(act, Act::NestedExplicit | Act::VariantCreate) => Some("menu"),
            COMMITTED => Some("committed"),
            AFTER => Some("after-undo"),
            _ => None,
        } {
            self.capture(editor, shell, gpu, act, stage)?;
        }
        if self.age == COMMITTED {
            self.committed = Some(self.observe_commit(act, editor)?);
        }
        if self.age >= AFTER {
            let after = fixture::snapshot(editor);
            let unchanged = after == self.before;
            let failed_apply = act == Act::FailedApply;
            let history_ok = editor.host.store().history().len()
                == self.before_history + if failed_apply { 0 } else { 2 };
            let revision_ok = editor.host.store().revision()
                == self.before_revision + if failed_apply { 0 } else { 2 };
            let committed_ok = self
                .committed
                .as_ref()
                .is_some_and(|value| value["ok"] == true);
            let diagnostics_ok = !failed_apply
                || self.diagnostics["entries"]
                    .as_array()
                    .is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry["diagnostic"]["message"]
                                .as_str()
                                .is_some_and(|message| {
                                    message.contains("outside the chosen prefab instance")
                                })
                        })
                    });
            let ok = unchanged && history_ok && revision_ok && committed_ok && diagnostics_ok;
            self.results.push(json!({"journey":act.label(),"ok":ok,"beforeRevision":self.before_revision,
                "afterRevision":editor.host.store().revision(),"allDocumentsRestored":unchanged,"oneActionOneUndo":history_ok&&revision_ok,
                "committed":self.committed,"diagnostics":self.diagnostics,"trace":self.trace,"capturePrefix":self.prefix(act)}));
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.waited = 0;
        self.trace.clear();
        self.committed = None;
        self.diagnostics = Value::Null;
    }
    fn prefix(&self, act: Act) -> String {
        format!("prefab-{:02}-{}", self.index + 1, act.label())
    }
    fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        act: Act,
        stage: &str,
    ) -> ara_ui::Result<()> {
        let name = format!("{}-{stage}", self.prefix(act));
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{name}.png")))?;
        std::fs::write(
            self.output.join(format!("{name}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        self.trace.push(json!({"stage":stage,"revision":editor.host.store().revision(),"selection":editor.state.selection,"documents":fixture::capture_documents(editor)}));
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("prefab.json"),
            ara_json::to_canonical_string(&json!({
                "documentsEncoding":"canonical-utf8-byte-length+blake3/v1",
                "journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len()
            })),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara native prefab evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}</style><h1>Native J5 prefab evidence</h1><p>Hidden native target; production pointer and keyboard routes. <a href='prefab.json'>Structured assertions</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "committed", "after-undo"] {
                let file = format!("prefab-{:02}-{}-{stage}.png", index + 1, act.label());
                html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("prefab-index.html"), html)?;
        Ok(())
    }
}
