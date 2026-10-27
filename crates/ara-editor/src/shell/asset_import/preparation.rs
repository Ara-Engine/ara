use super::*;

impl Prepared {
    pub(super) fn from_plan(
        mut request: Request,
        selection: Selection,
        plan: backend::Plan,
    ) -> ara_ui::Result<Self> {
        request.conflicts = plan.labels();
        Ok(Self {
            request,
            selection,
            plan: plan.value()?,
        })
    }
}
impl Shell {
    fn prepare_asset_import(
        &mut self,
        editor: &mut Editor,
        request: Request,
    ) -> ara_ui::Result<()> {
        let selection = Selection::read(editor);
        match backend::preflight(editor, &request)? {
            backend::Preflight::Ready(plan) => self
                .asset_import
                .ready
                .push_back(Prepared::from_plan(request, selection, plan)?),
            backend::Preflight::Pending(id) => self.asset_import.jobs.push(Job {
                id,
                destination: request.destination.clone(),
                sources: request.sources.len(),
                selection,
                status: backend::Status::preparing(),
                last: Instant::now(),
                cancel_requested: false,
                preparation: Some(request),
                plan: None,
                error: None,
            }),
        }
        Ok(())
    }
    fn start_asset_import(
        &mut self,
        editor: &mut Editor,
        prepared: Prepared,
        conflict: &str,
    ) -> ara_ui::Result<()> {
        let Prepared {
            request,
            selection,
            plan,
        } = prepared;
        let sources: Vec<_> = request
            .sources
            .iter()
            .map(|path| {
                path.to_str()
                    .ok_or("An import source cannot be represented as UTF-8")
            })
            .collect::<Result<_, _>>()?;
        let why = format!(
            "Import {} file{}",
            sources.len(),
            if sources.len() == 1 { "" } else { "s" }
        );
        // The ordinary session's default marker anchors imports without an open scene.
        let accepted = editor.action("cmd.apply",json!({"why":why,"commands":[{
            "cmd":"asset.import","args":{"sources":sources,"destination":request.destination,"conflict":conflict}
        }]}))?;
        let job = backend::accepted_job(&accepted)
            .ok_or("asset.import did not return its background job")?;
        self.asset_import.jobs.push(Job {
            id: job.into(),
            destination: request.destination,
            sources: request.sources.len(),
            status: backend::Status::preparing(),
            selection,
            last: Instant::now(),
            cancel_requested: false,
            preparation: None,
            plan: Some(plan),
            error: None,
        });
        Ok(())
    }
    fn advance_import_choices(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        let choice = self
            .asset_import
            .choice
            .as_mut()
            .and_then(|choice| choice.response.try_take());
        if let Some(choice) = choice {
            let pending = self
                .asset_import
                .choice
                .take()
                .ok_or("Import conflict choice lost its request")?;
            if let Some(policy) =
                choice.and_then(|index| ["rename", "replace", "skip"].get(index).copied())
            {
                if let Err(error) = self.start_asset_import(editor, pending.prepared, policy) {
                    self.alert_error(editor, &*error);
                }
            } else {
                self.status_note = Some("Import cancelled".into());
            }
            changed = true;
        }
        for _ in 0..8 {
            if self.asset_import.choice.is_some() {
                break;
            }
            if self.confirmations.is_open()
                && self
                    .asset_import
                    .ready
                    .front()
                    .is_some_and(|prepared| !prepared.request.conflicts.is_empty())
            {
                break;
            }
            let Some(prepared) = self.asset_import.ready.pop_front() else {
                break;
            };
            if prepared.request.conflicts.is_empty() {
                if let Err(error) = self.start_asset_import(editor, prepared, "rename") {
                    self.alert_error(editor, &*error);
                }
            } else {
                let count = prepared.request.conflicts.len();
                let mut paths = prepared
                    .request
                    .conflicts
                    .iter()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                if count > 6 {
                    paths.push_str(&format!("\n… and {} more", count - 6));
                }
                let options = DialogOptions::new(
                    "Files already exist",
                    format!(
                        "{count} destination path{} already exist.\n\n{paths}\n\nRename keeps both copies. Replace updates existing assets. Skip leaves existing files unchanged.",
                        if count == 1 { "" } else { "s" }
                    ),
                );
                let response = self.confirmations.choose(
                    options,
                    vec!["Rename".into(), "Replace".into(), "Skip".into()],
                    0,
                );
                self.asset_import.choice = Some(Choice { prepared, response });
                self.menu = None;
            }
            changed = true;
        }
        Ok(changed)
    }
    /// Read-only planning can return a background job. Neither its completion nor
    /// its conflicts alter selection; only an actual successful import can do so.
    pub(crate) fn poll_asset_imports(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        for _ in 0..8 {
            let Some(request) = self.asset_import.queued.pop_front() else {
                break;
            };
            if let Err(error) = self.prepare_asset_import(editor, request) {
                self.alert_error(editor, &*error);
            }
            changed = true;
        }
        changed |= self.poll_import_job_status(editor)?;
        changed |= self.advance_import_choices(editor)?;
        if changed {
            self.asset_import.revision += 1;
        }
        Ok(changed)
    }
}
