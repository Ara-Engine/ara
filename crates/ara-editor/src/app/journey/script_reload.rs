//! J10 runtime-effect proof, separate from the journey's retained-input setup.
mod runtime;
mod source;
use crate::Editor;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

pub(in crate::app) const SPEED: f64 = 30.;
const FASTER: f64 = SPEED + 240.;
const HZ: f64 = 60.;
const DEADLINE: Duration = Duration::from_secs(180);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Stage {
    Changed,
    Broken,
    Fixed,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Broken => "broken",
            Self::Fixed => "fixed",
        }
    }
}
enum Phase {
    Baseline,
    Write,
    Reload,
    Measure,
    Done,
}

pub(in crate::app) struct Proof {
    doc: String,
    entity: String,
    path: String,
    world: Option<String>,
    original: String,
    faster: String,
    broken: String,
    current: String,
    stage: Option<Stage>,
    phase: Phase,
    began: Instant,
    probe: runtime::Probe,
    write_job: Option<String>,
    next_source: Option<String>,
    module: Option<u64>,
    reload: Option<Value>,
    error: Option<Value>,
    error_cursor: u64,
    measurements: BTreeMap<String, Value>,
    events: Vec<Value>,
    writes: Vec<Value>,
    authoring_transform: Value,
    history_retained: bool,
}
impl Proof {
    pub fn new(editor: &Editor, path: &Path, entity: &str) -> ara_ui::Result<Self> {
        let relative = path
            .strip_prefix(editor.host.root())?
            .to_str()
            .ok_or("script path is not UTF-8")?
            .replace('\\', "/");
        let original = std::fs::read_to_string(path)?;
        if original.matches("Speed * tick.Dt").count() != 1 {
            return Err("Spinner template has no unique rotation-rate expression".into());
        }
        let faster = original.replacen("Speed * tick.Dt", "(Speed + 240f) * tick.Dt", 1);
        let broken = format!("{faster}\n#error ARA_J10_INVALID_SOURCE\n");
        let view = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|e| format!("{e:?}"))?;
        let record = &view.get(entity).ok_or("Spinner target is missing")?.record;
        if !record.contains_key("Spinner") {
            return Err("Spinner was not actually attached to the proof entity".into());
        }
        Ok(Self {
            doc: editor.state.doc.clone(),
            entity: entity.into(),
            path: relative,
            world: None,
            current: original.clone(),
            original,
            faster,
            broken,
            stage: None,
            phase: Phase::Baseline,
            began: Instant::now(),
            probe: Default::default(),
            write_job: None,
            next_source: None,
            module: None,
            reload: None,
            error: None,
            error_cursor: 0,
            measurements: Default::default(),
            events: Vec::new(),
            writes: Vec::new(),
            authoring_transform: record.get("Transform").cloned().unwrap_or(Value::Null),
            history_retained: false,
        })
    }

    pub fn run(&mut self, stage: Stage, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.stage != Some(stage) {
            self.stage = Some(stage);
            self.began = Instant::now();
            self.reload = None;
            self.probe = Default::default();
            let world = editor
                .play_world
                .clone()
                .ok_or("J10 requires a live Play world")?;
            if self.world.as_ref().is_some_and(|old| *old != world) {
                return Err("J10 world changed instead of reloading in place".into());
            }
            self.world = Some(world);
            let _ = editor.action("play.events", json!({"world":self.world}))?;
            if stage == Stage::Changed {
                self.phase = Phase::Baseline
            } else {
                if stage == Stage::Broken {
                    self.error_cursor = editor.action("logs", json!({"limit":1}))?["cursor"]
                        .as_u64()
                        .unwrap_or(0);
                }
                let text = if stage == Stage::Broken {
                    self.broken.clone()
                } else {
                    self.faster.clone()
                };
                self.queue_write(editor, text)?;
            }
        }
        if self.began.elapsed() > DEADLINE {
            return Err(format!(
                "J10 {} timed out; last structured evidence {}",
                stage.name(),
                self.report()
            )
            .into());
        }
        if editor.play_world.as_ref() != self.world.as_ref() {
            return Err("J10 Play world disappeared or restarted".into());
        }
        self.poll_events(editor)?;
        let world = self.world.as_deref().ok_or("J10 world missing")?;
        match self.phase {
            Phase::Baseline => {
                if let Some(measured) = self.probe.poll(editor, world, &self.entity, SPEED / HZ)? {
                    self.check_measurement("baseline", measured)?;
                    self.queue_write(editor, self.faster.clone())?;
                }
            }
            Phase::Write => {
                if let Some(result) = runtime::job(editor, &mut self.write_job)? {
                    self.writes.push(json!({"completed":result}));
                    self.current = self
                        .next_source
                        .take()
                        .ok_or("J10 source job has no proposed text")?;
                    self.phase = Phase::Reload;
                }
            }
            Phase::Reload => {
                if let Some(reload) = &self.reload {
                    let errors = reload["errors"]
                        .as_u64()
                        .ok_or("reload event has no compiler error count")?;
                    if stage == Stage::Broken {
                        self.collect_error(editor)?;
                        if errors > 0 && self.error.is_some() {
                            self.phase = Phase::Measure
                        }
                    } else if errors == 0 {
                        self.phase = Phase::Measure
                    }
                }
            }
            Phase::Measure => {
                if let Some(measured) = self.probe.poll(editor, world, &self.entity, FASTER / HZ)? {
                    self.check_measurement(stage.name(), measured)?;
                    if stage == Stage::Fixed {
                        self.history_retained = false;
                        self.collect_error(editor)?;
                        if !self.history_retained {
                            return Err(
                                "Fixed compile erased or lost the historical error diagnostic"
                                    .into(),
                            );
                        }
                    }
                    self.check_authoring(editor)?;
                    self.phase = Phase::Done;
                }
            }
            Phase::Done => {
                if stage == Stage::Broken {
                    self.collect_error(editor)?
                }
                return Ok(true);
            }
        }
        Ok(matches!(self.phase, Phase::Done))
    }

    fn queue_write(&mut self, editor: &mut Editor, text: String) -> ara_ui::Result<()> {
        self.reload = None;
        let (job, proof) = source::replace(
            editor,
            &self.doc,
            &self.path,
            &self.current,
            &text,
            &format!(
                "J10 {} Spinner runtime effect",
                self.stage.map_or("changed", Stage::name)
            ),
        )?;
        self.writes.push(proof);
        self.write_job = Some(job);
        self.next_source = Some(text);
        self.phase = Phase::Write;
        Ok(())
    }
    fn poll_events(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let response = editor.action("play.events", json!({"world":self.world}))?;
        for event in response["events"].as_array().into_iter().flatten() {
            if event["kind"] != "reload" {
                continue;
            }
            let module = event["module"]
                .as_u64()
                .ok_or("reload has no module slot")?;
            if self.module.is_some_and(|expected| module != expected) {
                continue;
            }
            let level = event["level"].as_u64().ok_or("reload has no level")?;
            if level == 0 {
                continue;
            }
            if matches!(self.phase, Phase::Baseline | Phase::Done) {
                continue;
            }
            if !(1..=3).contains(&level) && level != 100 {
                return Err(format!("J10 requires in-place reload, received level {level}").into());
            }
            if self.events.len() >= 64 {
                return Err("J10 reload event evidence exceeded its bounded inventory".into());
            }
            self.events.push(event.clone());
            self.module.get_or_insert(module);
            self.reload = Some(event.clone());
        }
        Ok(())
    }
    fn collect_error(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let logs = editor.action(
            "logs",
            json!({"since":self.error_cursor,"severity":"error","group":false,"limit":1000}),
        )?;
        for entry in logs["entries"].as_array().into_iter().flatten() {
            let diagnostic = &entry["diagnostic"];
            if !diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("ARA_J10_INVALID_SOURCE"))
            {
                continue;
            }
            if !diagnostic["location"]["file"]
                .as_str()
                .is_some_and(|p| p.replace('\\', "/").ends_with("/Spinner.cs") || p == "Spinner.cs")
            {
                continue;
            }
            if self.error.is_none() {
                self.error = Some(entry.clone())
            }
            if self.error.as_ref().is_some_and(|old| {
                old["first"] == entry["first"] && old["diagnostic"] == entry["diagnostic"]
            }) {
                self.history_retained = true;
                self.error = Some(entry.clone());
            }
        }
        Ok(())
    }
    fn check_measurement(&mut self, name: &str, measurement: Value) -> ara_ui::Result<()> {
        let passed = measurement["passed"] == true;
        self.measurements.insert(name.into(), measurement);
        if !passed {
            return Err(format!(
                "J10 {name} real ECS rotation rate did not match: {}",
                self.measurements[name]
            )
            .into());
        }
        Ok(())
    }
    fn check_authoring(&self, editor: &Editor) -> ara_ui::Result<()> {
        let view =
            crate::world::resolve(editor.host.store(), &self.doc).map_err(|e| format!("{e:?}"))?;
        let current = view
            .get(&self.entity)
            .and_then(|entity| entity.record.get("Transform"))
            .cloned()
            .unwrap_or(Value::Null);
        if current != self.authoring_transform {
            return Err("Runtime rotation leaked into authored Transform".into());
        }
        Ok(())
    }
    pub fn error_sequence(&self) -> Option<u64> {
        self.error.as_ref()?["seq"].as_u64()
    }
    pub fn report(&self) -> Value {
        json!({"entity":self.entity,"source":self.path,"world":self.world,"module":self.module,
            "stage":self.stage.map(Stage::name),"measurements":self.measurements,"reloads":self.events,
            "sourceWrites":self.writes,"compileErrorHistory":self.error,"historicalErrorRetained":self.history_retained,
            "currentCompileErrorCount":self.reload.as_ref().and_then(|r|r["errors"].as_u64()),
            "currentCompileStateScope":"Latest structured reload event for the same runtime module, not a global Problems active-error API",
            "fixedSourceProved":self.stage==Some(Stage::Fixed)&&matches!(self.phase,Phase::Done),
            "baselineSourceBytes":self.original.len(),"ingress":"Source.replace jobs and background ECS observations are protocol proof steps; journey setup separately records retained widget ingress"})
    }
}
