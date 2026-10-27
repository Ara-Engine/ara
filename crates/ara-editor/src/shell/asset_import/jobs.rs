use super::*;

impl Shell {
    pub(super) fn poll_import_job_status(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        let mut index = 0;
        while index < self.asset_import.jobs.len() {
            let job = &self.asset_import.jobs[index];
            if !job.status.running() || job.last.elapsed() < Duration::from_millis(100) {
                index += 1;
                continue;
            }
            let result = editor.action("jobs.status", json!({"job":job.id}))?;
            let status = backend::Status::read(result);
            let job = &mut self.asset_import.jobs[index];
            job.last = Instant::now();
            if status == job.status {
                index += 1;
                continue;
            }
            job.status = status;
            changed = true;
            if job.preparation.is_some() && job.status.succeeded() {
                let plan = backend::Plan::read(&job.status.value()["result"]);
                match plan {
                    Ok(plan) => {
                        let job = self.asset_import.jobs.remove(index);
                        let request = job.preparation.ok_or("Import preflight lost its request")?;
                        self.asset_import.ready.push_back(Prepared::from_plan(
                            request,
                            job.selection,
                            plan,
                        )?);
                        continue;
                    }
                    Err(error) => {
                        self.asset_import.jobs[index].error = Some(error.to_string());
                        self.alert_error(editor, &*error);
                    }
                }
            } else if job.preparation.is_none() && job.status.succeeded() {
                self.status_note = Some(job.status.summary());
                // Only an explicit storage path can navigate the browser. Preserve
                // a selection the user changed while planning/importing proceeded.
                if job.selection.unchanged(editor)
                    && let Some(path) = job.status.selection_path()
                {
                    editor.action("editor.workspace", json!({"asset":path}))?;
                }
            } else if !job.status.running() {
                self.status_note = Some(job.status.summary());
            }
            index += 1;
        }
        Ok(changed)
    }
}
