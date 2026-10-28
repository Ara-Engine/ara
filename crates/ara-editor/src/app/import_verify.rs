//! Native OS-drop import proof on the verifier's scratch project and profile.
//! Preparation and conflict planning stay in the backend; this probe observes their real jobs.
use super::input_verify::native as native_input;
use super::*;
use ara_host::project::{AuthoredRole, AuthoredScope};
use std::{collections::BTreeSet, path::Path};
mod fixture;
mod recovery;
mod stream;

const FOLDER: &str = "P4 Import Proof";
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    DragEscape,
    DragFocusLost,
    ClosedTarget,
    Batch,
    Rename,
    Replace,
    Skip,
    Cancel,
    UndoRedo,
}
const ACTS: &[Act] = &[
    Act::DragEscape,
    Act::DragFocusLost,
    Act::ClosedTarget,
    Act::Batch,
    Act::Rename,
    Act::Replace,
    Act::Skip,
    Act::Cancel,
    Act::UndoRedo,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Batch => "folder-multi-file-batch",
            Self::Rename => "conflict-rename",
            Self::Replace => "conflict-replace",
            Self::Skip => "conflict-skip",
            Self::Cancel => "cancel-preparation",
            Self::UndoRedo => "one-import-undo-redo",
            Self::DragEscape => "native-drag-escape-late-files",
            Self::DragFocusLost => "native-drag-focus-loss-late-files",
            Self::ClosedTarget => "native-drag-detached-target-close",
        }
    }
    fn choice(self) -> Option<usize> {
        match self {
            Self::Rename => Some(0),
            Self::Replace => Some(1),
            Self::Skip => Some(2),
            _ => None,
        }
    }
    fn discarded(self) -> bool {
        matches!(
            self,
            Self::DragEscape | Self::DragFocusLost | Self::ClosedTarget
        )
    }
    fn streamed(self) -> bool {
        self == Self::Batch || self.discarded()
    }
}
struct Target {
    storage: String,
    parent: String,
}
pub(super) struct ImportCheck {
    output: PathBuf,
    inputs: fixture::Inputs,
    folder_job: String,
    target: Option<Target>,
    index: usize,
    age: u32,
    waiting: bool,
    failed_frame: bool,
    started: Instant,
    before: Value,
    committed: Option<Value>,
    selection: Value,
    old_jobs: BTreeSet<String>,
    before_history: usize,
    before_revision: u64,
    terminal: Option<Value>,
    cancel_observed: Option<Value>,
    results: Vec<Value>,
    trace: Vec<Value>,
    injected_events: u64,
    initialized: bool,
    stream_target: Option<(u32, [f32; 2])>,
    stream_frames: u32,
    recovering: bool,
    cleanup_events: u64,
    cleanup_capture_pending: bool,
    cleanup_controls_captured: bool,
}
impl ImportCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        let inputs = fixture::Inputs::new(output, editor.host.root())?;
        let reply = editor.action(
            "cmd.apply",
            json!({"doc":"ara.project.json","why":"Create isolated native import proof folder",
            "commands":[{"cmd":"folder.create","args":{"path":FOLDER,"parents":true}}]}),
        )?;
        let job = reply["job"]
            .as_str()
            .or_else(|| reply["outputs"][0]["job"].as_str())
            .filter(|job| !job.is_empty())
            .ok_or("folder.create did not return its preparation job")?
            .to_owned();
        Ok(Self {
            output: output.into(),
            inputs,
            folder_job: job,
            target: None,
            index: 0,
            age: 0,
            waiting: false,
            failed_frame: false,
            started: Instant::now(),
            before: Value::Null,
            committed: None,
            selection: Value::Null,
            old_jobs: BTreeSet::new(),
            before_history: 0,
            before_revision: 0,
            terminal: None,
            cancel_observed: None,
            results: Vec::new(),
            trace: Vec::new(),
            injected_events: 0,
            initialized: false,
            stream_target: None,
            stream_frames: 0,
            recovering: false,
            cleanup_events: 0,
            cleanup_capture_pending: false,
            cleanup_controls_captured: false,
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len() && !self.recovering
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
        floats: &windows::Floats,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.recovering {
            let before = events.len();
            let cleaned = self.recover(shell, window, scale, events)?;
            self.cleanup_events += events.len().saturating_sub(before) as u64;
            if cleaned {
                self.recovering = false;
                // The next case has not taken its baseline or initialized its
                // layout yet; do not let this cleanup frame capture age zero.
                self.failed_frame = true;
                self.started = Instant::now();
                if let Some(result) = self.results.last_mut() {
                    result["cleanup"] = json!({"jobsDrained":true,"ownedCardsDismissed":true,"conflictDialogClosed":true,"nativeEvents":self.cleanup_events});
                }
                self.write()?;
            }
            return Ok(());
        }
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        let existing = events.len();
        if let Err(error) = self.input(act, editor, shell, window, scale, floats, events) {
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace,"imports":shell.asset_import_report(),"injectedEvents":self.injected_events}));
            events.clear();
            events.push((window.id(), Event::FocusLost));
            if let Some((sdl, _)) = self.stream_target {
                events.push((sdl, Event::FocusLost));
                events.push((sdl, Event::FileDropComplete));
            }
            key(window, 41, events)?;
            self.advance();
            self.failed_frame = true;
            self.write()?;
        } else {
            let injected = &events[existing..];
            self.injected_events += injected.len() as u64;
            if !injected.is_empty() {
                self.trace.push(json!({"age":self.age,"events":injected.iter().map(|(_,event)|format!("{event:?}")).collect::<Vec<_>>()}));
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        floats: &windows::Floats,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !shell.offscreen {
            return Err("Native import proof requires the isolated offscreen editor".into());
        }
        if self.started.elapsed() > Duration::from_secs(120) {
            return Err(format!(
                "Import {} timed out before its authoritative outcome",
                act.label()
            )
            .into());
        }
        if self.target.is_none() {
            let status = editor.action("jobs.status", json!({"job":self.folder_job}))?;
            match status["status"].as_str() {
                Some("done") => {}
                Some("running") => return self.wait(),
                _ => {
                    return Err(format!(
                        "Import fixture folder failed: {}",
                        ara_json::to_canonical_string(&status)
                    )
                    .into());
                }
            }
            let folder = shell.asset_authored_files().iter().find(|file| {
                matches!(file.scope, AuthoredScope::Authored)
                    && matches!(file.role, AuthoredRole::Folder)
                    && file.path == FOLDER
            });
            let Some(folder) = folder else {
                return self.wait();
            };
            let parent = folder
                .storage_path
                .rsplit_once('/')
                .map_or(String::new(), |(parent, _)| parent.to_owned());
            self.target = Some(Target {
                storage: folder.storage_path.clone(),
                parent,
            });
        }
        let target = self.target.as_ref().ok_or("Import proof target missing")?;
        if self.age > 0 && act.streamed() && (self.age <= 4 || act.discarded()) {
            return self.stream_input(act, editor, shell, window, scale, floats, events);
        }
        let stage = if act == Act::Batch && self.age >= 5 {
            self.age - 1
        } else {
            self.age
        };
        match stage {
            0 => {
                if !self.initialized {
                    shell.leave();
                    shell.menu = None;
                    shell.dialog = None;
                    editor.action("editor.layout.reset", json!({}))?;
                    editor.action("editor.focus", json!({"panel":"assets"}))?;
                    editor.action("editor.workspace", json!({"folder":target.parent}))?;
                    editor.action("editor.search", json!({"panel":"assets","query":""}))?;
                    if act == Act::ClosedTarget {
                        if !floats.hidden {
                            return Err(
                                "The detached import proof requires hidden native windows".into()
                            );
                        }
                        editor.action(
                            "editor.layout.move",
                            json!({"panel":"assets","slot":"float","rect":[120,100,760,680]}),
                        )?;
                    }
                    if matches!(act, Act::Replace | Act::Skip) {
                        self.inputs
                            .change_tone(if act == Act::Replace { 2 } else { 3 })?;
                    }
                    shell.rebuild(editor)?;
                    self.initialized = true;
                }
                if self
                    .drop_point(act, editor, shell, window, scale, floats)?
                    .is_none()
                {
                    return self.wait();
                }
                self.selection = fixture::selection(editor, shell);
                self.before = fixture::snapshot(editor, &target.storage)?;
                self.before_revision = editor.host.store().revision();
                self.before_history = editor.host.store().history().len();
                self.old_jobs = job_ids(&shell.asset_import_report());
            }
            1 => {
                let point = folder_point(shell, &target.storage)
                    .ok_or("The indexed import folder is not reachable in Assets")?
                    .map(|value| value / scale);
                let sources = match act {
                    Act::Batch => vec![&self.inputs.tone, &self.inputs.other, &self.inputs.tone],
                    Act::Cancel => vec![&self.inputs.large],
                    Act::UndoRedo => vec![&self.inputs.undo],
                    _ => vec![&self.inputs.tone],
                };
                for source in sources {
                    stream::native(
                        window.id(),
                        Event::FileDrop {
                            path: source.clone(),
                            position: point,
                        },
                        events,
                    )?;
                }
            }
            2 => {
                let report = shell.asset_import_report();
                if act.choice().is_some() {
                    if report["choosingConflict"] != true {
                        if import_job(&report, &self.old_jobs).is_some() {
                            return Err(
                                "A known conflict imported without its shared choice".into()
                            );
                        }
                        return self.wait();
                    }
                } else if report["choosingConflict"] == true {
                    return Err("A routine OS import opened an unnecessary conflict dialog".into());
                } else if act == Act::Cancel {
                    let Some(job) = import_job(&report, &self.old_jobs) else {
                        return self.wait();
                    };
                    let status = &job["record"];
                    if status["status"] != "running" {
                        return Err("Preparation completed before a cancelable native frame; cancellation remains unverified".into());
                    }
                    if status.get("cancellable").is_none() {
                        // The just-created frontend card is not a jobs.status
                        // receipt. Never infer a commit boundary from its placeholder.
                        return self.wait();
                    }
                    if !cancellable(status) {
                        return Err(
                            "Import reached committing before cancellation could be observed"
                                .into(),
                        );
                    }
                    let id = job["job"]
                        .as_str()
                        .ok_or("Cancelable import job identity missing")?;
                    let action =
                        format!("asset-import-{:016x}-action", ara_ui::source::stable_id(id));
                    if ready_control(shell, &action, window, scale, events)?.is_none() {
                        return self.wait();
                    }
                    self.cancel_observed = Some(status.clone());
                }
                if fixture::selection(editor, shell) != self.selection {
                    return Err("Selection changed before an import succeeded".into());
                }
            }
            3 => {
                if let Some(choice) = act.choice() {
                    let Some(id) = choice_control(shell, choice) else {
                        return self.wait();
                    };
                    if !click(shell, &id, window, scale, events)? {
                        return self.wait();
                    }
                } else if act == Act::Cancel {
                    let report = shell.asset_import_report();
                    let job = import_job(&report, &self.old_jobs)
                        .ok_or("Cancelable import disappeared")?;
                    if !cancellable(&job["record"]) {
                        return Err("Preparation committed before the native Cancel click; cancellation remains unverified".into());
                    }
                    let id = job["job"].as_str().ok_or("Import job identity missing")?;
                    if !click(
                        shell,
                        &format!("asset-import-{:016x}-action", ara_ui::source::stable_id(id)),
                        window,
                        scale,
                        events,
                    )? {
                        return self.wait();
                    }
                }
            }
            4 => {
                let report = shell.asset_import_report();
                if act == Act::Batch && report["choosingConflict"] == true {
                    return Err(
                        "A routine streamed import opened an unnecessary conflict dialog".into(),
                    );
                }
                let Some(job) = import_job(&report, &self.old_jobs) else {
                    return self.wait();
                };
                if let Some(error) = job["frontendError"].as_str() {
                    return Err(error.to_owned().into());
                }
                match job["record"]["status"].as_str() {
                    Some("running") => {
                        if fixture::selection(editor, shell) != self.selection {
                            return Err(
                                "Selection changed while import preparation was running".into()
                            );
                        }
                        return self.wait();
                    }
                    Some("done") if act != Act::Cancel => {}
                    Some("cancelled") if act == Act::Cancel => {}
                    _ => {
                        return Err(format!(
                            "Import finished unexpectedly: {}",
                            ara_json::to_canonical_string(job)
                        )
                        .into());
                    }
                }
                let committed = fixture::snapshot(editor, &target.storage)?;
                if !matches!(act, Act::Cancel | Act::Skip) {
                    let media = committed["files"]
                        .as_object()
                        .into_iter()
                        .flat_map(|files| files.keys())
                        .filter(|path| path.ends_with(".wav"))
                        .count();
                    if committed["identities"]
                        .as_object()
                        .map_or(0, serde_json::Map::len)
                        != media
                    {
                        return self.wait();
                    }
                }
                self.terminal = Some(job.clone());
                self.committed = Some(committed);
            }
            5 if act == Act::UndoRedo => chord(window, false, events)?,
            6 if act == Act::UndoRedo => {
                if fixture::snapshot(editor, &target.storage)? != self.before {
                    return self.wait();
                }
            }
            7 if act == Act::UndoRedo => chord(window, true, events)?,
            8 if act == Act::UndoRedo => {
                if self.committed.as_ref() != Some(&fixture::snapshot(editor, &target.storage)?) {
                    return self.wait();
                }
            }
            5..=8 => self.age = if act == Act::Batch { 10 } else { 9 },
            _ => {}
        }
        Ok(())
    }
    fn wait(&mut self) -> ara_ui::Result<()> {
        self.waiting = true;
        Ok(())
    }
    /// Inspect the completed hidden native target after production event routing and presentation.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.recovering && self.cleanup_capture_pending {
            let act = ACTS
                .get(self.index.saturating_sub(1))
                .ok_or("Cleanup capture has no owning import case")?;
            let name = format!("import-{:02}-{}-cleanup-controls", self.index, act.label());
            gpu.select_target(0);
            gpu.pixels()?
                .save(self.output.join(format!("{name}.png")))?;
            std::fs::write(
                self.output.join(format!("{name}.layout.json")),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
            self.cleanup_capture_pending = false;
            self.cleanup_controls_captured = true;
            if let Some(result) = self.results.last_mut() {
                result["cleanupControlCapture"] = json!({"prefix":name,"nativeTarget":0});
            }
            self.write()?;
        }
        if std::mem::take(&mut self.failed_frame)
            || self.finished()
            || self.recovering
            || self.waiting
        {
            return Ok(());
        }
        let act = ACTS[self.index];
        let age = if act == Act::Batch && self.age >= 5 {
            self.age - 1
        } else {
            self.age
        };
        if let Some(stage) = match age {
            0 => Some("before"),
            1 => Some("dropped"),
            2 => Some("choice-or-preparation"),
            3 if act.streamed() => Some("stream-retained"),
            4 if act == Act::Batch && self.age == 4 => Some("complete-submitted"),
            4 => Some("completed"),
            6 if act == Act::UndoRedo => Some("undo"),
            9 => Some("after"),
            _ => None,
        } {
            self.capture(editor, shell, gpu, act, stage)?;
        }
        if age >= 9 {
            let target = self.target.as_ref().ok_or("Import target missing")?;
            let after = fixture::snapshot(editor, &target.storage)?;
            let terminal = self.terminal.as_ref().ok_or("Import outcome missing")?;
            let outcome = crate::shell::asset_import::completed_output(&terminal["record"]);
            let unchanged = after == self.before;
            let transactions = editor
                .host
                .store()
                .history()
                .len()
                .saturating_sub(self.before_history);
            let new_jobs = job_ids(&shell.asset_import_report())
                .difference(&self.old_jobs)
                .count();
            let ok = match act {
                Act::DragEscape | Act::DragFocusLost | Act::ClosedTarget => {
                    unchanged
                        && transactions == 0
                        && new_jobs == 0
                        && fixture::selection(editor, shell) == self.selection
                        && shell.file_drag_report().is_null()
                }
                Act::Skip => {
                    unchanged
                        && transactions == 0
                        && fixture::selection(editor, shell) == self.selection
                        && count(outcome, "skipped") == 1
                }
                Act::Cancel => {
                    unchanged
                        && transactions == 0
                        && fixture::selection(editor, shell) == self.selection
                        && self.cancel_observed.is_some()
                }
                Act::Batch => {
                    !unchanged
                        && transactions == 1
                        && new_jobs == 1
                        && self.stream_frames >= 3
                        && terminal["destination"] == FOLDER
                        && terminal["sources"] == 2
                        && paired_created(
                            outcome,
                            &after["identities"],
                            &["native-other.wav", "native-tone.wav"],
                        )
                        && after["identities"]
                            .as_object()
                            .is_some_and(|ids| ids.len() == 2)
                }
                Act::Rename => {
                    !unchanged
                        && transactions == 1
                        && count(outcome, "renamed") == 1
                        && after["files"]["native-tone.wav"]
                            == self.before["files"]["native-tone.wav"]
                        && after["identities"]["native-tone.wav"]
                            == self.before["identities"]["native-tone.wav"]
                        && after["identities"]
                            .as_object()
                            .map_or(0, serde_json::Map::len)
                            == self.before["identities"]
                                .as_object()
                                .map_or(0, serde_json::Map::len)
                                + 1
                }
                Act::Replace => {
                    !unchanged
                        && transactions == 1
                        && count(outcome, "replaced") == 1
                        && after["files"]["native-tone.wav"]
                            != self.before["files"]["native-tone.wav"]
                        && after["identities"]["native-tone.wav"]
                            == self.before["identities"]["native-tone.wav"]
                }
                Act::UndoRedo => {
                    !unchanged && transactions == 3 && self.committed.as_ref() == Some(&after)
                }
            };
            self.results.push(json!({"journey":act.label(),"ok":ok,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),
                "transactions":transactions,"unchanged":unchanged,"selectionBefore":self.selection,"selectionAfter":fixture::selection(editor,shell),
                "cancelablePreparation":self.cancel_observed,"requiredResultInventoryPresent":outcome.get("created").is_some()||outcome.get("skipped").is_some(),
                "outcome":terminal,"before":self.before,"after":after,"trace":self.trace,"injectedEvents":self.injected_events,"newImportJobs":new_jobs,"framesRetainedBeforeComplete":self.stream_frames,
                "createdMediaAndPairedSidecars":if act==Act::Batch {Some(paired_created(outcome,&after["identities"],&["native-other.wav","native-tone.wav"]))}else{None}}));
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
        self.started = Instant::now();
        self.waiting = false;
        self.trace.clear();
        self.terminal = None;
        self.cancel_observed = None;
        self.committed = None;
        self.injected_events = 0;
        self.initialized = false;
        self.stream_target = None;
        self.stream_frames = 0;
        self.recovering = true;
        self.cleanup_events = 0;
        self.cleanup_capture_pending = false;
        self.cleanup_controls_captured = false;
    }
    fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        act: Act,
        stage: &str,
    ) -> ara_ui::Result<()> {
        let name = format!("import-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{name}.png")))?;
        std::fs::write(
            self.output.join(format!("{name}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        self.trace.push(json!({"stage":stage,"selection":fixture::selection(editor,shell),"imports":shell.asset_import_report(),"fileDrag":shell.file_drag_report(),"revision":editor.host.store().revision()}));
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("os-imports.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"journeys":self.results,
            "passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len(),"fixtures":self.inputs.measurements}),
            ),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara native OS import evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}</style><h1>Native OS import evidence</h1><p>Hidden native window, real FileDrop events and authority jobs. <a href='os-imports.json'>Structured outcomes and replayable event traces</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "choice-or-preparation", "after"] {
                let file = format!("import-{:02}-{}-{stage}.png", index + 1, act.label());
                if self.output.join(&file).is_file() {
                    html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
                }
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("os-imports-index.html"), html)?;
        Ok(())
    }
}
fn count(result: &Value, key: &str) -> usize {
    result[key].as_array().map_or(0, Vec::len)
}
/// Import inventory lists media and its sidecar separately; require both paths
/// exactly once, with the same freshly indexed identity for each pair.
fn paired_created(result: &Value, identities: &Value, expected: &[&str]) -> bool {
    let Some(created) = result["created"].as_array() else {
        return false;
    };
    created.len() == expected.len() * 2
        && expected.iter().all(|name| {
            let Some(id) = identities[*name].as_str() else {
                return false;
            };
            let media = format!("{FOLDER}/{name}");
            let sidecar = format!("{media}.import.json");
            [&media, &sidecar].into_iter().all(|path| {
                created
                    .iter()
                    .filter(|entry| {
                        entry["path"].as_str() == Some(path.as_str())
                            && entry["asset"].as_str() == Some(id)
                    })
                    .count()
                    == 1
            })
        })
}
fn job_ids(report: &Value) -> BTreeSet<String> {
    report["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|job| job["job"].as_str().map(str::to_owned))
        .collect()
}
fn import_job<'a>(report: &'a Value, old: &BTreeSet<String>) -> Option<&'a Value> {
    report["jobs"].as_array()?.iter().find(|job| {
        job["preflight"] == false && job["job"].as_str().is_some_and(|id| !old.contains(id))
    })
}
fn cancellable(status: &Value) -> bool {
    crate::shell::asset_import::preparation_cancellable(status)
}
fn folder_point(shell: &Shell, path: &str) -> Option<[f32; 2]> {
    shell.ui.nodes.iter().enumerate().find_map(|(index,node)| {
    if !shell.ui.shown(index) {return None}
    if matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(crate::shell::Binding::Asset(asset)) if asset.path==path&&asset.kind=="folder") {
        let point=[node.rect.x+node.rect.w*0.5,node.rect.y+node.rect.h*0.5];
        node.clip.contains(point[0],point[1]).then_some(point)
    } else {None}
})
}
/// Reveal an enabled control with native scrolling before observing or clicking it.
fn ready_control(
    shell: &Shell,
    id: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<Option<ara_ui::paint::Box2>> {
    let Some(node) = shell.shown_node(id).map(|index| &shell.ui.nodes[index]) else {
        return Ok(None);
    };
    if node.state.disabled {
        return Ok(None);
    }
    let rect = node.rect.intersection(node.clip);
    if node.rect.w <= 2.
        || node.rect.h <= 2.
        || rect.w < node.rect.w * 0.99
        || rect.h < node.rect.h * 0.99
    {
        // Finished cards remain reviewable in a real scrolling surface. Use its
        // normal wheel path to reach an owned job instead of clicking through a clip.
        if id.starts_with("asset-import-")
            && let Some(index) = shell.shown_node("asset-jobs-scroll")
        {
            let scroll = &shell.ui.nodes[index];
            let rect = scroll.rect.intersection(scroll.clip);
            if rect.w > 2. && rect.h > 2. {
                native_input::motion(
                    window,
                    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
                    scale,
                    events,
                )?;
                native_input::wheel(
                    window,
                    if node.rect.y < rect.y { 0.25 } else { -0.25 },
                    events,
                )?;
            }
        }
        return Ok(None);
    }
    Ok(Some(rect))
}
fn click(
    shell: &Shell,
    id: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<bool> {
    let Some(rect) = ready_control(shell, id, window, scale, events)? else {
        return Ok(false);
    };
    native_input::motion(
        window,
        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
        scale,
        events,
    )?;
    native_input::button(window, true, events)?;
    native_input::button(window, false, events)?;
    Ok(true)
}
fn key(window: &Window, scan: i32, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    native_input::tap(window, scan, events)
}
fn chord(window: &Window, redo: bool, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    native_input::key(window, primary, true, events)?;
    if redo {
        native_input::key(window, 225, true, events)?;
    }
    key(window, 29, events)?;
    if redo {
        native_input::key(window, 225, false, events)?;
    }
    native_input::key(window, primary, false, events)
}

fn choice_control(shell: &Shell, choice: usize) -> Option<String> {
    let modal = shell.ui.nodes.iter().find(|node| {
        node.element.tag == "modal"
            && node
                .element
                .attrs
                .get("label")
                .is_some_and(|title| title == "Files already exist")
            && shell.shown_node(&node.element.id).is_some()
    })?;
    let id = format!("{}-choice-{choice}", modal.element.id);
    shell.shown_node(&id).map(|_| id)
}
