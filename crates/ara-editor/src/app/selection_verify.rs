//! J2/J3 selection gestures through the production native event router, on the behaviour fixture.
use super::*;
use ara_ui::paint::Box2;
use std::{collections::BTreeSet, path::Path};
mod fixture;
mod input;
mod observe;

const ROWS: usize = 120;
const MESHES: [&str; 3] = ["e_proofmesh1", "e_proofmesh2", "e_proofmesh3"];
const HIDDEN: &str = "e_proofhidden";
const LOCKED: &str = "e_prooflocked";
const BEFORE: u32 = 6;
const PREVIEW: u32 = 12;
const AFTER: u32 = 18;
const PATIENCE: u32 = 144 * 120;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    ShiftClick,
    CtrlToggle,
    CtrlShift,
    KeyboardEscape,
    DeletedAnchor,
    MarqueeReplace,
    MarqueeAdd,
    MarqueeSubtract,
    MarqueeReverse,
    MarqueeEscape,
    Duplicate,
    DuplicateEscape,
}
const ACTS: &[Act] = &[
    Act::ShiftClick,
    Act::CtrlToggle,
    Act::CtrlShift,
    Act::KeyboardEscape,
    Act::DeletedAnchor,
    Act::MarqueeReplace,
    Act::MarqueeAdd,
    Act::MarqueeSubtract,
    Act::MarqueeReverse,
    Act::MarqueeEscape,
    Act::Duplicate,
    Act::DuplicateEscape,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::ShiftClick => "hierarchy-shift-range",
            Self::CtrlToggle => "hierarchy-control-toggle",
            Self::CtrlShift => "hierarchy-control-shift-union",
            Self::KeyboardEscape => "hierarchy-virtual-range-escape",
            Self::DeletedAnchor => "hierarchy-deleted-anchor",
            Self::MarqueeReplace => "marquee-replace",
            Self::MarqueeAdd => "marquee-add",
            Self::MarqueeSubtract => "marquee-subtract",
            Self::MarqueeReverse => "marquee-reverse",
            Self::MarqueeEscape => "marquee-escape",
            Self::Duplicate => "alt-duplicate-undo-redo",
            Self::DuplicateEscape => "alt-duplicate-escape",
        }
    }
    fn hierarchy(self) -> bool {
        matches!(
            self,
            Self::ShiftClick
                | Self::CtrlToggle
                | Self::CtrlShift
                | Self::KeyboardEscape
                | Self::DeletedAnchor
        )
    }
    fn duplicate(self) -> bool {
        matches!(self, Self::Duplicate | Self::DuplicateEscape)
    }
    fn finish(self) -> u32 {
        if self == Self::Duplicate { 28 } else { AFTER }
    }
}
fn row(index: usize) -> String {
    format!("e_selection{index:04}")
}

pub(super) struct SelectionCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    failed_frame: bool,
    from: [f32; 2],
    to: [f32; 2],
    before_revision: u64,
    before_records: Value,
    committed_records: Option<Value>,
    trace: Vec<Value>,
    results: Vec<Value>,
}
impl SelectionCheck {
    /// Create authored fixtures only in the isolated project opened by the behaviour route.
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Selection verification requires the isolated behaviour fixture".into());
        }
        std::fs::create_dir_all(output)?;
        fixture::create(editor)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            failed_frame: false,
            from: [0.; 2],
            to: [0.; 2],
            before_revision: 0,
            before_records: Value::Null,
            committed_records: None,
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
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
            .collect()
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "native route owners are borrowed independently"
    )]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        scene: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if let Err(error) = self.input(act, editor, shell, window, scale, scene, events) {
            shell.leave();
            shell.desktop_pointer = None;
            events.clear();
            events.push((0, Event::FocusLost));
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    /// Capture completed native targets. Assertions inspect authored state, not the preview patch.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
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
        let stage = match self.age {
            BEFORE => Some("before"),
            PREVIEW => Some("preview"),
            AFTER if act == Act::Duplicate => Some("committed"),
            22 if act == Act::Duplicate => Some("undo"),
            age if age == act.finish() => Some("after"),
            _ => None,
        };
        if let Some(stage) = stage {
            self.capture(editor, shell, gpu, act, stage)?;
            if stage == "committed" {
                self.committed_records = Some(fixture::records(editor)?);
            }
        }
        if self.age >= act.finish() {
            let (ok, observed) = self
                .observe(act, editor, shell)
                .unwrap_or_else(|error| (false, json!({"error":error.to_string()})));
            self.results.push(json!({"journey":act.label(),"ok":ok,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),"observed":observed,"trace":self.trace,"capturePrefix":self.prefix(act)}));
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
        self.committed_records = None;
    }
    fn prefix(&self, act: Act) -> String {
        format!("selection-{:02}-{}", self.index + 1, act.label())
    }
    fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        act: Act,
        stage: &str,
    ) -> ara_ui::Result<()> {
        let prefix = format!("{}-{stage}", self.prefix(act));
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{prefix}.png")))?;
        std::fs::write(
            self.output.join(format!("{prefix}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        let focused = shell
            .input
            .focused()
            .and_then(|id| shell.bindings.get(&id))
            .and_then(|binding| {
                if let crate::shell::Binding::Select(entity) = binding {
                    Some(entity.as_str())
                } else {
                    None
                }
            });
        let preview = editor.gesture.as_ref().map(|gesture| json!({"base":gesture.base,"roots":gesture.current.iter().map(|(entity,matrix)|(entity,matrix.to_cols_array())).collect::<std::collections::BTreeMap<_,_>>()}));
        self.trace.push(json!({"stage":stage,"selection":editor.state.selection,"focusedEntity":focused,"hierarchyOffset":shell.hierarchy_offset,"pointer":shell.pointer,"from":self.from,"to":self.to,"revision":editor.host.store().revision(),"gesture":preview,"records":fixture::records(editor)?}));
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("selection.json"),
            ara_json::to_canonical_string(
                &json!({"journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len()}),
            ),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara native selection evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}</style><h1>Native J2/J3 selection evidence</h1><p>Hidden native window, production event routing. <a href='selection.json'>Structured outcomes</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "preview", "after"] {
                let file = format!("selection-{:02}-{}-{stage}.png", index + 1, act.label());
                html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("selection-index.html"), html)?;
        Ok(())
    }
}
