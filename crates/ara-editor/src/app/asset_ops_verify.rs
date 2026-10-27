//! Native Assets stories use real fields, menus, drags and shared confirmations.
//! Setup is typed authoring in the isolated behaviour project; probes are read-only.
use super::*;
use ara_host::project::{AuthoredRole, AuthoredScope};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
mod fixture;
mod input;
mod observe;
const ROOT: &str = "P4 Asset Proof";
const FINISH: u32 = 18;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Rename,
    RenameCancel,
    Move,
    MoveConflict,
    InvalidFolder,
    Duplicate,
    DeleteCancel,
    Delete,
    SafeDelete,
    Folder,
    CancelPreparation,
    Index,
}
const ACTS: &[Act] = &[
    Act::Rename,
    Act::RenameCancel,
    Act::Move,
    Act::MoveConflict,
    Act::InvalidFolder,
    Act::Duplicate,
    Act::DeleteCancel,
    Act::Delete,
    Act::SafeDelete,
    Act::Folder,
    Act::CancelPreparation,
    Act::Index,
];
impl Act {
    fn folder(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::RenameCancel => "Rename Escape",
            Self::Move => "Move",
            Self::MoveConflict => "Move Conflict",
            Self::InvalidFolder => "Cycle",
            Self::Duplicate => "Duplicate",
            Self::DeleteCancel => "Delete Cancel",
            Self::Delete => "Delete",
            Self::SafeDelete => "Safe Delete",
            Self::Folder => "Create Folder",
            Self::CancelPreparation => "Cancel",
            Self::Index => "Index",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Rename => "inline-rename-id-reference-undo",
            Self::RenameCancel => "inline-rename-escape",
            Self::Move => "multi-folder-drop-one-undo",
            Self::MoveConflict => "multi-move-conflict-atomic",
            Self::InvalidFolder => "folder-self-drop-invalid",
            Self::Duplicate => "context-duplicate-fresh-ids",
            Self::DeleteCancel => "context-delete-cancel",
            Self::Delete => "context-delete-one-undo",
            Self::SafeDelete => "inbound-reference-safe-delete-blocker",
            Self::Folder => "context-create-empty-folder",
            Self::CancelPreparation => "cancel-duplicate-preparation",
            Self::Index => "semantic-browser-scope-projection",
        }
    }
    fn path(self) -> String {
        format!("{ROOT}/{}", self.folder())
    }
    fn multi(self) -> bool {
        matches!(
            self,
            Self::Move
                | Self::MoveConflict
                | Self::Duplicate
                | Self::DeleteCancel
                | Self::Delete
                | Self::SafeDelete
        )
    }
    fn drag(self) -> bool {
        matches!(self, Self::Move | Self::MoveConflict | Self::InvalidFolder)
    }
    fn undo(self) -> bool {
        matches!(
            self,
            Self::Rename | Self::Move | Self::Duplicate | Self::Delete | Self::Folder
        )
    }
    fn no_job(self) -> bool {
        matches!(
            self,
            Self::RenameCancel | Self::DeleteCancel | Self::InvalidFolder | Self::Index
        )
    }
    fn fails(self) -> bool {
        matches!(self, Self::MoveConflict | Self::SafeDelete)
    }
    fn source(self) -> String {
        match self {
            Self::InvalidFolder | Self::CancelPreparation => format!("{}/Folder", self.path()),
            _ => format!("{}/A.prefab", self.path()),
        }
    }
}
pub(super) struct AssetOpsCheck {
    output: PathBuf,
    fixture: fixture::Fixture,
    index: usize,
    age: u32,
    waiting: bool,
    failed_frame: bool,
    began: Instant,
    before: Value,
    committed: Option<Value>,
    undo: Option<Value>,
    terminal: Option<Value>,
    selection: Value,
    selection_paths: Value,
    committed_selection: Option<Value>,
    undo_selection: Option<Value>,
    redo_selection: Option<Value>,
    unrelated_selection: Option<Value>,
    before_revision: u64,
    before_history: usize,
    old_job: Option<String>,
    cancel_observed: Option<Value>,
    drag_preview: Value,
    index_preview: Value,
    pointer: [f32; 2],
    source: [f32; 2],
    destination: [f32; 2],
    initialized: bool,
    recovering: bool,
    results: Vec<Value>,
    trace: Vec<Value>,
    events: u64,
}
impl AssetOpsCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        let fixture = fixture::Fixture::new(editor, output)?;
        Ok(Self {
            output: output.into(),
            fixture,
            index: 0,
            age: 0,
            waiting: false,
            failed_frame: false,
            began: Instant::now(),
            before: Value::Null,
            committed: None,
            undo: None,
            terminal: None,
            selection: Value::Null,
            selection_paths: Value::Null,
            committed_selection: None,
            undo_selection: None,
            redo_selection: None,
            unrelated_selection: None,
            before_revision: 0,
            before_history: 0,
            old_job: None,
            cancel_observed: None,
            drag_preview: Value::Null,
            index_preview: Value::Null,
            pointer: [0.; 2],
            source: [0.; 2],
            destination: [0.; 2],
            initialized: false,
            recovering: false,
            results: Vec::new(),
            trace: Vec::new(),
            events: 0,
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
        let initial = events.len();
        let result = (|| {
            if !shell.offscreen {
                return Err(
                    "Asset operations proof requires the isolated offscreen behaviour editor"
                        .into(),
                );
            }
            if !scale.is_finite() || scale <= 0. {
                return Err("Asset operations proof has invalid native scale".into());
            }
            if self.began.elapsed() > Duration::from_secs(180) {
                return Err(format!("Native asset story {} timed out", act.label()).into());
            }
            if self.recovering {
                if shell.asset_operation_report()["busy"] == true {
                    return self.wait();
                }
                self.recovering = false;
            }
            if !self.fixture.prepare(editor, shell, act)? {
                return self.wait();
            }
            self.input(act, editor, shell, window, scale, events)
        })();
        if let Err(error) = result {
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"age":self.age,"trace":self.trace,"assetOperation":shell.asset_operation_report(),"events":self.events}));
            events.clear();
            events.push((window.id(), Event::FocusLost));
            input::tap(window, 41, events);
            let report = shell.asset_operation_report();
            if report["busy"] == true
                && let Some(job) = report["job"]["id"]
                    .as_str()
                    .filter(|job| Some(*job) != self.old_job.as_deref())
            {
                let _ = editor.action("jobs.cancel", json!({"job":job}));
            }
            self.advance();
            self.recovering = report["busy"] == true;
            self.failed_frame = true;
            self.write()?;
        } else {
            self.events += (events.len() - initial) as u64;
            if events.len() > initial {
                self.trace.push(json!({"age":self.age,"events":events[initial..].iter().map(|(_,event)|format!("{event:?}")).collect::<Vec<_>>()}));
            }
        }
        Ok(())
    }
    fn wait(&mut self) -> ara_ui::Result<()> {
        self.waiting = true;
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.waiting = false;
        self.began = Instant::now();
        self.before = Value::Null;
        self.committed = None;
        self.undo = None;
        self.terminal = None;
        self.selection = Value::Null;
        self.old_job = None;
        self.cancel_observed = None;
        self.drag_preview = Value::Null;
        self.index_preview = Value::Null;
        self.initialized = false;
        self.trace.clear();
        self.events = 0;
        self.selection_paths = Value::Null;
        self.committed_selection = None;
        self.undo_selection = None;
        self.redo_selection = None;
        self.unrelated_selection = None;
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("asset-operations.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"journeys":self.results,"passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len()}),
            ),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara native Assets evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}</style><h1>Native Assets behaviour evidence</h1><p>Hidden native window, real input, typed authority jobs. <a href='asset-operations.json'>Outcomes, per-item diagnostics and input traces</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "preview", "after"] {
                let file = format!("assets-{:02}-{}-{stage}.png", index + 1, act.label());
                if self.output.join(&file).is_file() {
                    html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
                }
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("asset-operations-index.html"), html)?;
        Ok(())
    }
}
