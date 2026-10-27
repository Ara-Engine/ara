//! Discover the actual typed catalog and probe it through ordinary cmd.apply.
mod cases;
mod jobs;
use super::{Action, Editor, Shell};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Default)]
enum Discovery {
    #[default]
    List,
    Describe,
    Jobs,
    Ready,
}
struct Command {
    name: String,
    schema: Value,
    example: Value,
}
#[derive(Default)]
struct Counts {
    attempts: u64,
    accepted: u64,
    committed: BTreeSet<String>,
    cases: BTreeSet<String>,
    changed: BTreeMap<String, usize>,
    rejected_diagnostics: BTreeMap<String, u64>,
}
#[derive(Default)]
pub(super) struct State {
    discovery: Discovery,
    names: BTreeSet<String>,
    commands: Vec<Command>,
    errors: Vec<String>,
    cursor: usize,
    pub generated: usize,
    counts: BTreeMap<String, Counts>,
    jobs: jobs::Jobs,
    events: Vec<Value>,
    catalog_pending: bool,
}
impl State {
    pub fn discovery_action(&self) -> Option<Action> {
        let (method, params) = match self.discovery {
            Discovery::List => ("schema.list", json!({"kind":"command"})),
            Discovery::Describe => ("schema.describe", json!({"names":self.names})),
            Discovery::Jobs => ("jobs.list", json!({})),
            Discovery::Ready => return None,
        };
        Some(Action::Protocol {
            method: method.into(),
            params,
            raw: false,
        })
    }
    pub fn pending(&self) -> bool {
        self.cursor < self.commands.len() * cases::NAMES.len()
    }
    pub fn next(&mut self, editor: &Editor) -> Option<Action> {
        if let Some(action) = self.jobs.next(false) {
            self.generated += 1;
            return Some(action);
        }
        let command = self.commands.get(self.cursor / cases::NAMES.len())?;
        let case = self.cursor % cases::NAMES.len();
        let (args, changed_fields) = cases::probe(&command.example, &command.schema, case);
        let action = Action::TypedCommand {
            name: command.name.clone(),
            doc: anchor(editor, &command.name),
            args,
            case: cases::NAMES[case].into(),
            changed_fields,
        };
        self.cursor += 1;
        self.generated += 1;
        Some(action)
    }
    /// None leaves native UI routing and unrelated protocol actions untouched.
    pub fn run(
        &mut self,
        action: &Action,
        editor: &mut Editor,
        shell: &mut Shell,
    ) -> Option<ara_ui::Result<()>> {
        if let Action::TypedCommand {
            name,
            doc,
            args,
            case,
            changed_fields,
        } = action
        {
            let count = self.counts.entry(name.clone()).or_default();
            count.attempts += 1;
            count.cases.insert(case.clone());
            count.changed.insert(case.clone(), *changed_fields);
            let result = editor.action("cmd.apply", json!({"doc":doc,"why":format!("Editor monkey: {name} ({case})"),"commands":[{"cmd":name,"args":args}]}));
            let event = match &result {
                Ok(output) => {
                    self.counts.entry(name.clone()).or_default().accepted += 1;
                    self.commit(name, output);
                    self.jobs.returned(name, output);
                    json!({"name":name,"case":case,"hostAccepted":true,"txn":output["txn"],"job":output["job"]})
                }
                Err(error) => {
                    let mut codes = Vec::new();
                    for diagnostic in error
                        .0
                        .data
                        .as_ref()
                        .and_then(|v| v["diagnostics"].as_array())
                        .into_iter()
                        .flatten()
                    {
                        if let Some(code) = diagnostic["code"].as_str() {
                            codes.push(code.to_owned());
                            *self
                                .counts
                                .entry(name.clone())
                                .or_default()
                                .rejected_diagnostics
                                .entry(code.into())
                                .or_default() += 1;
                        }
                    }
                    json!({"name":name,"case":case,"hostAccepted":false,"rpcCode":error.0.code,"diagnosticCodes":codes})
                }
            };
            self.events.push(event);
            return Some(
                result
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
                    .and_then(|_| shell.rebuild(editor)),
            );
        }
        let Action::Protocol {
            method,
            params,
            raw: false,
        } = action
        else {
            return None;
        };
        let discover = matches!(
            (&self.discovery, method.as_str()),
            (Discovery::List, "schema.list")
                | (Discovery::Describe, "schema.describe")
                | (Discovery::Jobs, "jobs.list")
        );
        let job = params["job"].as_str().filter(|id| self.jobs.owns(id));
        let owned_job = job.is_some() && matches!(method.as_str(), "jobs.status" | "jobs.cancel");
        if !discover && !owned_job {
            return None;
        }
        let result = editor.action(method, params.clone());
        match result {
            Ok(output) => {
                if discover {
                    self.discovered(&output);
                }
                if let Some(job) = job
                    && let Some(command) = self.jobs.observe(job, &output)
                {
                    self.commit(&command, &output["result"]);
                    self.events.push(json!({"job":job,"command":command,"status":output["status"],"txn":output["result"]["txn"]}));
                }
                Some(shell.rebuild(editor))
            }
            Err(error) => {
                if discover {
                    self.errors.push(format!("{method}: {error}"));
                    self.advance_discovery();
                }
                Some(Err(Box::new(error)))
            }
        }
    }
    fn commit(&mut self, name: &str, output: &Value) {
        if let Some(txn) = output["txn"].as_str() {
            self.counts
                .entry(name.into())
                .or_default()
                .committed
                .insert(txn.into());
        }
    }
    fn discovered(&mut self, response: &Value) {
        match self.discovery {
            Discovery::List => {
                if let Some(items) = response["items"].as_array() {
                    for item in items {
                        if item["kind"] == "command" {
                            if let Some(name) = item["name"].as_str() {
                                self.names.insert(name.into());
                            } else {
                                self.errors.push("Command catalog entry has no name".into());
                            }
                        }
                    }
                    if self.names.is_empty() {
                        self.errors.push("Typed command catalog is empty".into());
                    }
                } else {
                    self.errors.push("schema.list returned no items".into());
                }
            }
            Discovery::Describe => {
                if let Some(items) = response["items"].as_array() {
                    let items: BTreeMap<_, _> = items
                        .iter()
                        .filter_map(|v| v["name"].as_str().map(|n| (n, v)))
                        .collect();
                    for name in &self.names {
                        let Some(item) = items.get(name.as_str()) else {
                            self.errors.push(format!("No descriptor for {name}"));
                            continue;
                        };
                        let Some(example) = item["examples"]
                            .as_array()
                            .and_then(|v| v.first())
                            .filter(|v| v.is_object())
                        else {
                            self.errors.push(format!("No object example for {name}"));
                            continue;
                        };
                        if !item["params"].is_object() {
                            self.errors.push(format!("No params schema for {name}"));
                            continue;
                        }
                        self.commands.push(Command {
                            name: name.clone(),
                            schema: item["params"].clone(),
                            example: example.clone(),
                        });
                    }
                } else {
                    self.errors.push("schema.describe returned no items".into());
                }
                self.catalog_pending = true;
            }
            Discovery::Jobs => {
                if let Err(error) = self.jobs.baseline(response) {
                    self.errors.push(error);
                }
            }
            Discovery::Ready => {}
        }
        self.advance_discovery();
    }
    fn advance_discovery(&mut self) {
        self.discovery = match self.discovery {
            Discovery::List => {
                if self.names.is_empty() {
                    Discovery::Jobs
                } else {
                    Discovery::Describe
                }
            }
            Discovery::Describe => Discovery::Jobs,
            Discovery::Jobs | Discovery::Ready => Discovery::Ready,
        };
    }
    pub fn take_catalog(&mut self) -> Option<Value> {
        std::mem::take(&mut self.catalog_pending).then(|| json!({"names":self.names,"errors":self.errors,"commands":self.commands.iter().map(|c| json!({"name":c.name,"params":c.schema,"example":c.example})).collect::<Vec<_>>()}))
    }
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
    pub fn drain(&mut self) -> Option<Action> {
        self.jobs.next(true)
    }
    pub fn drained(&self) -> bool {
        self.jobs.drained()
    }
    pub fn complete(&self) -> bool {
        matches!(self.discovery, Discovery::Ready)
            && self.errors.is_empty()
            && self.missing().is_empty()
            && !self.names.is_empty()
    }
    fn missing(&self) -> Vec<Value> {
        self.names
            .iter()
            .filter_map(|name| {
                let seen = self.counts.get(name);
                let missing: Vec<_> = cases::NAMES
                    .iter()
                    .filter(|case| seen.is_none_or(|c| !c.cases.contains(**case)))
                    .collect();
                (!missing.is_empty()).then(|| json!({"command":name,"cases":missing}))
            })
            .collect()
    }
    pub fn report(&self) -> Value {
        json!({"typedCommandCatalogReady":matches!(self.discovery, Discovery::Ready),"typedCommandCatalog":self.names,
            "typedCommandCatalogErrors":self.errors,"typedCommandCoverageComplete":self.complete(),"missingTypedCommandEdgeCases":self.missing(),
            "typedCommandCases":cases::NAMES,"typedCommandCounts":self.counts.iter().map(|(name,c)| (name,json!({"attempts":c.attempts,"accepted":c.accepted,"committedTransactions":c.committed.len(),"cases":c.cases,"changedFields":c.changed,"rejectedDiagnostics":c.rejected_diagnostics}))).collect::<BTreeMap<_,_>>(),
            "typedCommandJobs":self.jobs.report(),"typedCommandCatalogFile":"monkey-typed-catalog.json",
            "typedCommandLimits":["Attempted probes can reject at schema, document-kind, permission or reference validation; accepted and confirmed transaction counts are separate.","Seven cases use the first registered example and its schema. changedFields=0 means the case has no applicable values; it does not invent numeric/reference fields.","NaN is a string in JSON probes, never an IEEE NaN; native numeric-field NaN text events remain in the UI sweep.","Anchors use available authored document kinds; examples can require objects absent in the scratch fixture."]})
    }
}

fn anchor(editor: &Editor, command: &str) -> String {
    let workspace = editor.host.store().workspace();
    let domain = if command.contains("animation") {
        "animation"
    } else if command.contains("material") {
        "material"
    } else if command.contains("vfx") {
        "vfx"
    } else if command.starts_with("graph.") || command.starts_with("editor.graph.") {
        "graph"
    } else {
        "scene"
    };
    workspace
        .docs()
        .map(|(_, d)| d)
        .find(|d| {
            if domain == "scene" || domain == "animation" {
                d.kind == domain
            } else {
                d.kind == "graph"
                    && (domain == "graph"
                        || d.record("g_meta")
                            .and_then(|r| r.get("domain"))
                            .and_then(Value::as_str)
                            == Some(domain))
            }
        })
        .map(|d| d.id.to_string())
        .or_else(|| workspace.find("ara.project.json").map(ToString::to_string))
        .unwrap_or_else(|| editor.state.doc.clone())
}
