//! Native entry points for existing diagnostic and asset-reference tools.
use super::*;
use ara_ui::dialogs::{DialogFuture, DialogOptions};
mod validation;
pub(crate) use validation::ValidationService;

struct Why {
    doc: String,
    response: DialogFuture<Option<String>>,
}
struct References {
    paths: Vec<String>,
    response: DialogFuture<Option<usize>>,
}
/// Pending read-only tool interaction, independent of authoring selection.
#[derive(Default)]
pub(crate) struct State {
    why: Option<Why>,
    references: Option<References>,
    validation: Option<String>,
    last_why: Option<Value>,
    last_references: Option<String>,
    last_validation: Option<Value>,
}

impl Shell {
    /// Explain an explicit diagnostic/question, or ask using the original scene context.
    pub(crate) fn explain_problem(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        if let commands::Target::Text(question) = target
            && !question.trim().is_empty()
        {
            let doc = editor.state.doc.clone();
            return self.show_explanation(editor, &doc, question);
        }
        if self.reachability.why.is_some() {
            return Err("Finish or cancel the current diagnostic question first".into());
        }
        let question = match target {
            commands::Target::Entity(entity) => format!("{entity} not visible"),
            commands::Target::Asset(path) => self.asset_reference(editor, path).unwrap_or_default(),
            _ => editor
                .state
                .selection
                .first()
                .map_or_else(String::new, |entity| format!("{entity} not visible")),
        };
        let response = self.confirmations.prompt(
            DialogOptions::new(
                "Explain a problem",
                "Enter a diagnostic code, asset reference, or a question about an entity.",
            ),
            question,
        );
        self.reachability.why = Some(Why {
            doc: editor.state.doc.clone(),
            response,
        });
        self.menu = None;
        self.rebuild(editor)
    }
    fn show_explanation(
        &mut self,
        editor: &mut Editor,
        doc: &str,
        question: &str,
    ) -> ara_ui::Result<()> {
        let report = editor.action("why", json!({"question":question,"doc":doc,"full":true}))?;
        let text = report["text"]
            .as_str()
            .ok_or("The diagnostic service returned no explanation")?;
        self.dialog = Some(Dialog::Info {
            title: "Why?".into(),
            lines: text.lines().map(str::to_owned).collect(),
            code: None,
        });
        self.reachability.last_why = Some(json!({"doc":doc,"question":question,"report":report}));
        self.menu = None;
        self.rebuild(editor)
    }
    /// Show the existing bidirectional Uses/Used by inspector for an authored asset.
    pub(crate) fn show_asset_references(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        if let Some(path) = self.asset_target(editor, target) {
            return self.open_reference_asset(editor, &path);
        }
        if self.reachability.references.is_some() {
            return Err("Finish or cancel the current reference selection first".into());
        }
        let mut options = self
            .browser
            .items
            .iter()
            .filter_map(|asset| {
                let logical = asset.authored_path.as_ref()?;
                if asset.kind == "folder" {
                    None
                } else {
                    Some((asset.path.clone(), logical.clone()))
                }
            })
            .collect::<Vec<_>>();
        options.sort_by(|a, b| a.1.cmp(&b.1));
        if options.is_empty() {
            return Err("No authored assets are available in the current project index".into());
        }
        let labels = options.iter().map(|(_, label)| label.clone()).collect();
        let paths = options.into_iter().map(|(path, _)| path).collect();
        let response = self.confirmations.choose(
            DialogOptions::new(
                "Find asset references",
                "Choose an authored asset to inspect its Uses and Used by links.",
            ),
            labels,
            0,
        );
        self.reachability.references = Some(References { paths, response });
        self.menu = None;
        self.rebuild(editor)
    }
    fn open_reference_asset(&mut self, editor: &mut Editor, path: &str) -> ara_ui::Result<()> {
        if !self.browser.items.iter().any(|asset| {
            asset.path == path && asset.authored_path.is_some() && asset.kind != "folder"
        }) {
            return Err("Choose an existing authored file to inspect its references".into());
        }
        editor.action("editor.workspace", json!({"asset":path}))?;
        editor.action("editor.focus", json!({"panel":"inspector"}))?;
        self.reachability.last_references = Some(path.to_owned());
        self.menu = None;
        self.rebuild(editor)
    }
    /// Validate a revision-stamped snapshot of loaded authoring documents on a worker.
    pub(crate) fn validate_authoring(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let result = editor.action("editor.validate", json!({}))?;
        let job = result["job"]
            .as_str()
            .ok_or("Validation did not return a job")?;
        self.reachability.validation = Some(job.to_owned());
        self.reachability.last_validation = Some(result);
        self.status_note = Some("Validating loaded authoring documents…".into());
        editor.action("editor.focus", json!({"panel":"problems"}))?;
        self.menu = None;
        self.rebuild(editor)
    }
    /// Poll shared dialogs and the existing job API without replacing authoring selection.
    pub(crate) fn poll_reachability(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        let why = self
            .reachability
            .why
            .as_mut()
            .and_then(|pending| pending.response.try_take());
        if let Some(answer) = why
            && let Some(pending) = self.reachability.why.take()
        {
            if let Some(question) = answer.filter(|question| !question.trim().is_empty())
                && let Err(error) = self.show_explanation(editor, &pending.doc, question.trim())
            {
                self.alert_error(editor, &*error);
            }
            changed = true;
        }
        let reference = self
            .reachability
            .references
            .as_mut()
            .and_then(|pending| pending.response.try_take());
        if let Some(answer) = reference
            && let Some(pending) = self.reachability.references.take()
        {
            if let Some(path) = answer.and_then(|index| pending.paths.get(index))
                && let Err(error) = self.open_reference_asset(editor, path)
            {
                self.alert_error(editor, &*error);
            }
            changed = true;
        }
        if let Some(job) = self.reachability.validation.as_deref() {
            let result = editor.action("jobs.status", json!({"job":job}));
            match result {
                Ok(status) if status["status"] == "running" => {}
                Ok(status) => {
                    self.reachability.validation = None;
                    let result = &status["result"];
                    self.status_note = Some(if status["status"] == "done" {
                        format!(
                            "Validated {} loaded documents at revision {}: {} errors, {} warnings{}",
                            result["checked"],
                            result["revision"],
                            result["errors"],
                            result["warnings"],
                            if result["stale"] == true {
                                ". Project changed; run validation again."
                            } else {
                                ". Existing parse failures remain in Problems."
                            }
                        )
                    } else {
                        format!(
                            "Validation {}. Details are available in Problems.",
                            status["status"].as_str().unwrap_or("did not complete")
                        )
                    });
                    self.reachability.last_validation = Some(status);
                    changed = true;
                }
                Err(error) => {
                    self.reachability.validation = None;
                    self.alert_error(editor, &error);
                    changed = true;
                }
            }
        }
        if changed {
            self.rebuild(editor)?;
        }
        Ok(changed)
    }
    /// Actual tool responses for automation; this never starts a query or changes a view.
    pub(crate) fn reachability_report(&self) -> Value {
        let links=self.ui.nodes.iter().filter_map(|node| {
            let direction=if node.element.id.starts_with("dependency-Uses-") {"uses"}
                else if node.element.id.starts_with("dependency-Used by-") {"usedBy"} else {return None};
            let Binding::Protocol("editor.workspace",args)=self.bindings.get(&ara_ui::source::stable_id(&node.element.id))? else{return None};
            Some(json!({"id":node.element.id,"direction":direction,"asset":args["asset"],"box":node.rect,"clip":node.clip}))
        }).collect::<Vec<_>>();
        json!({"whyPending":self.reachability.why.is_some(),"why":self.reachability.last_why,
            "referencesPending":self.reachability.references.is_some(),"references":self.reachability.last_references,
            "referenceLinks":links,
            "validationJob":self.reachability.validation,"validation":self.reachability.last_validation})
    }
}
