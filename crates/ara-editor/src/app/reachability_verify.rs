//! Why, references and validation through real SDL palette/dialog/Inspector controls.
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PREFAB: &str = "assets/reachability.prefab";
const BASE: &str = "assets/box.prefab";
const ENTITY: &str = "e_reachability01";
const QUESTION: &str = "ARA1102";
const CASES: [&str; 3] = [
    "why-native-prompt",
    "reference-viewer-bidirectional-links",
    "validation-read-only-job",
];
const PATIENCE: Duration = Duration::from_secs(60);

pub(super) struct ReachabilityCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    phase_started: Instant,
    revision: u64,
    content: blake3::Hash,
    selection: Vec<String>,
    camera: crate::state::Camera,
    doc: String,
    scene: String,
    native_events: usize,
    result: Option<Value>,
    capture: Option<&'static str>,
    results: Vec<Value>,
    observations: Vec<Value>,
    previous_jobs: Vec<String>,
    job: Option<String>,
}
impl ReachabilityCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Reachability verification requires the isolated behaviour fixture".into());
        }
        if editor.gesture.is_some() {
            return Err("The preceding journey left an authoring gesture active".into());
        }
        let base = editor
            .host
            .store()
            .workspace()
            .index()
            .id_of(BASE)
            .ok_or("Reachability base prefab is missing")?;
        let reference = format!("@{BASE}#{base}");
        editor.apply(vec![json!({"cmd":"prefab.instantiate","args":{"prefab":reference,"id":ENTITY,"name":"Reachability reference fixture","position":[0,0,0]}})],"Create reachability reference fixture",None)?;
        editor.apply(vec![json!({"cmd":"prefab.variant","args":{"entity":ENTITY,"path":PREFAB,"id":"pf_reachability"}})],"Create bidirectional reference fixture",None)?;
        let scene = editor
            .host
            .store()
            .workspace()
            .index()
            .path_of(&editor.state.doc)
            .ok_or("Reachability scene identity is missing")?
            .to_owned();
        let previous_jobs = validation_jobs(editor)?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            phase_started: Instant::now(),
            revision: editor.host.store().revision(),
            content: content(editor),
            selection: editor.state.selection.clone(),
            camera: editor.state.camera,
            doc: editor.state.doc.clone(),
            scene,
            native_events: 0,
            result: None,
            capture: None,
            results: Vec::new(),
            observations: Vec::new(),
            previous_jobs,
            job: None,
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
        if self.finished() {
            return Ok(());
        }
        if !shell.offscreen {
            return Err("Reachability proof requires hidden offscreen windows".into());
        }
        let before = events.len();
        let result = self.step(editor, shell, window, scale, events);
        self.native_events += events.len() - before;
        if let Err(error) = &result {
            self.results.push(json!({"journey":CASES[self.index],"ok":false,"error":error.to_string(),"phase":self.phase,"tools":shell.reachability_report()}));
            self.write()?;
        }
        result
    }
    fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.phase_started.elapsed() >= PATIENCE {
            return Err(format!(
                "Reachability {} phase {} did not become reachable within {} real seconds",
                CASES[self.index],
                self.phase,
                PATIENCE.as_secs()
            )
            .into());
        }
        if self.phase == 0 {
            // Choice dialogs own a fixed option set. Wait for the ordinary
            // browser publication before opening it, rather than guessing IO time.
            if self.index == 1
                && (storage(shell, PREFAB).is_none()
                    || storage(shell, BASE).is_none()
                    || storage(shell, &self.scene).is_none())
            {
                return Ok(());
            }
            if self.index == 0 {
                if shell.modal_active() || editor.gesture.is_some() {
                    return Err("The preceding journey left a modal or gesture active".into());
                }
                shell.leave();
                editor.action("editor.layout.reset", json!({}))?;
                editor.action("editor.workspace", json!({"asset":""}))?;
                shell.rebuild(editor)?;
                native::focus_lost(window, events)?;
            }
            self.capture = Some("before");
            self.next();
        } else if self.phase == 1 {
            native::shortcut(window, 14, events)?; // Ctrl+K opens the production palette.
            self.next();
        } else if self.phase == 2 {
            if shell.shown_node("palette-search").is_none() {
                return Ok(());
            }
            native::shortcut(window, 4, events)?;
            native::text(window, self.command(), events)?;
            self.next();
        } else if self.phase == 3 {
            let id = shell
                .command_bindings()
                .into_iter()
                .find(|(id, command)| {
                    id.starts_with("command-")
                        && *command == self.command()
                        && shell.shown_node(id).is_some()
                })
                .map(|(id, _)| id);
            let Some(id) = id else { return Ok(()) };
            click(shell, window, scale, &id, events)?;
            self.observations
                .push(json!({"stage":"palette-ingress","command":self.command(),"control":id}));
            self.next();
        } else {
            match self.index {
                0 => self.why(editor, shell, window, scale, events)?,
                1 => self.references(editor, shell, window, scale, events)?,
                _ => self.validation(editor, shell)?,
            }
        }
        Ok(())
    }
    fn command(&self) -> &'static str {
        match self.index {
            0 => "tools.why",
            1 => "tools.references",
            _ => "assets.validate",
        }
    }
    fn next(&mut self) {
        self.phase += 1;
        self.phase_started = Instant::now();
    }
    fn why(
        &mut self,
        _editor: &mut Editor,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            4 => {
                let field = shell
                    .ui
                    .nodes
                    .iter()
                    .find(|node| {
                        node.element.id.starts_with("editor-confirm-")
                            && node.element.id.ends_with("-field")
                            && shell.shown_node(&node.element.id).is_some()
                    })
                    .map(|node| node.element.id.clone());
                let Some(field) = field else { return Ok(()) };
                click(shell, window, scale, &field, events)?;
                self.capture = Some("question-dialog");
                self.next();
            }
            5 => {
                native::shortcut(window, 4, events)?;
                native::text(window, QUESTION, events)?;
                self.next();
            }
            6 => {
                native::tap(window, 40, events)?;
                self.next();
            }
            7 => {
                let report = shell.reachability_report();
                let Some(text) = report["why"]["report"]["text"].as_str() else {
                    return Ok(());
                };
                if text.trim().is_empty() || !text.contains(QUESTION) {
                    return Err(
                        "Why returned no explanation for the requested real diagnostic code".into(),
                    );
                }
                if report["why"]["doc"] != self.doc || report["why"]["question"] != QUESTION {
                    return Err("Why answered a different question or document context".into());
                }
                let rendered = shell
                    .ui
                    .nodes
                    .iter()
                    .filter(|node| {
                        node.element.id.starts_with("dialog-line-")
                            && shell.shown_node(&node.element.id).is_some()
                    })
                    .filter_map(|node| node.text.as_ref().map(|text| text.value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if rendered != text.lines().collect::<Vec<_>>().join("\n") {
                    return Ok(());
                }
                self.observations
                    .push(json!({"stage":"why-answer","report":report,"renderedText":rendered}));
                self.capture = Some("explanation");
                self.next();
            }
            8 => {
                native::tap(window, 41, events)?;
                self.next();
            }
            _ => {
                if shell.shown_node("dialog").is_none() && !shell.modal_active() {
                    self.result = Some(json!({"actualExplanationShown":true,"question":QUESTION}));
                }
            }
        }
        Ok(())
    }
    fn references(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(prefab) = storage(shell, PREFAB) else {
            return Ok(());
        };
        let Some(base) = storage(shell, BASE) else {
            return Ok(());
        };
        let Some(scene) = storage(shell, &self.scene) else {
            return Ok(());
        };
        let report = shell.reachability_report();
        match self.phase {
            4 => {
                let choice = shell
                    .ui
                    .nodes
                    .iter()
                    .find(|node| {
                        node.element.id.starts_with("editor-confirm-")
                            && node.element.id.contains("-choice-")
                            && node.text.as_ref().is_some_and(|text| text.value == PREFAB)
                    })
                    .map(|node| node.element.id.clone());
                let Some(choice) = choice else { return Ok(()) };
                if !visible(shell, &choice) {
                    let list = shell
                        .ui
                        .nodes
                        .iter()
                        .find(|node| {
                            node.element.id.starts_with("editor-confirm-")
                                && node.element.id.ends_with("-choices")
                        })
                        .ok_or("Reference chooser has no native scroll surface")?;
                    let rect = list.rect.intersection(list.clip);
                    native::motion(
                        window,
                        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
                        scale,
                        events,
                    )?;
                    let node = &shell.ui.nodes[shell
                        .ui
                        .find(&choice)
                        .ok_or("Reference choice disappeared")?];
                    native::wheel(
                        window,
                        if node.rect.y < node.clip.y { 3. } else { -3. },
                        events,
                    )?;
                    return Ok(());
                }
                click(shell, window, scale, &choice, events)?;
                self.next();
            }
            5 => {
                if link(&report, "uses", base).is_none() {
                    return Ok(());
                }
                if report["references"] != prefab || link(&report, "usedBy", scene).is_none() {
                    return Ok(());
                }
                self.observations
                    .push(json!({"stage":"variant-both-directions","report":report}));
                self.capture = Some("bidirectional-links");
                self.next();
            }
            6 => {
                let Some(control) = link(&report, "uses", base) else {
                    return Ok(());
                };
                click(shell, window, scale, control, events)?;
                self.next();
            }
            7 => {
                if editor.state.asset_path.as_deref() != Some(base)
                    || link(&report, "usedBy", prefab).is_none()
                {
                    return Ok(());
                }
                self.observations
                    .push(json!({"stage":"base-used-by-variant","report":report}));
                self.capture = Some("reference-link-navigation");
                self.next();
            }
            8 => {
                let Some(control) = link(&report, "usedBy", prefab) else {
                    return Ok(());
                };
                click(shell, window, scale, control, events)?;
                self.next();
            }
            _ => {
                if editor.state.asset_path.as_deref() == Some(prefab)
                    && link(&report, "uses", base).is_some()
                    && link(&report, "usedBy", scene).is_some()
                {
                    self.result = Some(
                        json!({"usesBase":base,"usedByScene":scene,"navigatedBothDirections":true,"report":report}),
                    );
                }
            }
        }
        Ok(())
    }
    fn validation(&mut self, editor: &mut Editor, shell: &Shell) -> ara_ui::Result<()> {
        let report = shell.reachability_report();
        let Some(job) = report["validation"]["job"].as_str() else {
            return Ok(());
        };
        if self.job.is_none() {
            self.job = Some(job.to_owned());
            self.capture = Some("validation-job");
        }
        if self.job.as_deref() != Some(job) {
            return Err("Validation replaced its recorded job unexpectedly".into());
        }
        let status = editor.action("jobs.status", json!({"job":job}))?;
        if status["status"] == "running" {
            return Ok(());
        }
        if status["status"] != "done" {
            return Err(format!("Native validation did not complete: {status}").into());
        }
        let result = &status["result"];
        if result["scope"] != "loadedAuthoringDocuments"
            || result["revision"] != self.revision
            || result["stale"] != false
            || !result["checked"].is_u64()
            || !result["errors"].is_u64()
            || !result["warnings"].is_u64()
        {
            return Err("Validation returned an incomplete or stale result".into());
        }
        let jobs = validation_jobs(editor)?;
        let new = jobs
            .iter()
            .filter(|job| !self.previous_jobs.contains(job))
            .collect::<Vec<_>>();
        if new.len() != 1 || new[0] != job {
            return Err("Native validation did not produce exactly one real read-only job".into());
        }
        self.result = Some(json!({"oneRealJob":true,"job":status,"tools":report}));
        Ok(())
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if let Some(stage) = self.capture.take() {
            self.capture(shell, gpu, stage)?;
        }
        if let Some(result) = self.result.take() {
            let checks = json!({"authoringUnchanged":editor.host.store().revision()==self.revision&&content(editor)==self.content,
                "selectionUnchanged":editor.state.selection==self.selection,"cameraUnchanged":editor.state.camera==self.camera,"documentUnchanged":editor.state.doc==self.doc});
            let ok = checks
                .as_object()
                .is_some_and(|checks| checks.values().all(|value| value == true));
            self.capture(shell, gpu, "after")?;
            self.results.push(json!({"journey":CASES[self.index],"ok":ok,"checks":checks,"observed":result,"observations":self.observations}));
            self.index += 1;
            self.phase = 0;
            self.phase_started = Instant::now();
            self.observations.clear();
            self.write()?;
        }
        Ok(())
    }
    fn capture(&self, shell: &Shell, gpu: &mut NativeUi, stage: &str) -> ara_ui::Result<()> {
        gpu.select_target(0);
        let stem = format!(
            "reachability-{:02}-{}-{stage}",
            self.index + 1,
            CASES[self.index]
        );
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("reachability.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),"nativeEvents":self.native_events,"ingress":"raw SDL palette shortcut, text and pointer -> production route; dialogs and existing Inspector links","journeys":self.results,"current":{"journey":CASES.get(self.index),"phase":self.phase,"phaseSeconds":self.phase_started.elapsed().as_secs_f64()}}),
            ),
        )?;
        Ok(())
    }
}
fn content(editor: &Editor) -> blake3::Hash {
    let mut hash = blake3::Hasher::new();
    for (path, doc) in editor.host.store().workspace().docs() {
        hash.update(path.as_bytes());
        hash.update(&[0]);
        hash.update(ara_doc::write(doc, editor.host.store().schema()).as_bytes());
        hash.update(&[0]);
    }
    hash.finalize()
}
fn validation_jobs(editor: &mut Editor) -> ara_ui::Result<Vec<String>> {
    let jobs = editor.action("jobs.list", json!({}))?;
    Ok(jobs["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|job| job["name"] == "Validate loaded authoring documents")
        .filter_map(|job| job["job"].as_str().map(str::to_owned))
        .collect())
}
fn storage<'a>(shell: &'a Shell, logical: &str) -> Option<&'a str> {
    shell
        .asset_authored_files()
        .iter()
        .find(|file| {
            matches!(&file.scope, ara_host::project::AuthoredScope::Authored)
                && file.path == logical
        })
        .map(|file| file.storage_path.as_str())
}
fn link<'a>(report: &'a Value, direction: &str, asset: &str) -> Option<&'a str> {
    report["referenceLinks"]
        .as_array()?
        .iter()
        .find(|link| link["direction"] == direction && link["asset"] == asset)?["id"]
        .as_str()
}
fn visible(shell: &Shell, id: &str) -> bool {
    shell.shown_node(id).is_some_and(|index| {
        let node = &shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        rect.w > 1. && rect.h >= node.rect.h * 0.9
    })
}
fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("Reachability control is not shown: {id}"))?;
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    if rect.w <= 1. || rect.h <= 1. {
        return Err(format!("Reachability control is clipped: {id}").into());
    }
    native::motion(
        window,
        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
        scale,
        events,
    )?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
