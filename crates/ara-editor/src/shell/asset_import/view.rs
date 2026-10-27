use super::super::commands::Target;
use super::*;

impl Shell {
    /// A bounded scroll surface for import jobs, appended by the Assets panel owner.
    /// Existing jobs.status message carries files/bytes even before optional detail fields exist.
    pub(crate) fn asset_import_markup(&mut self, out: &mut String) {
        if self.asset_import.jobs.is_empty()
            && self.asset_import.queued.is_empty()
            && self.asset_import.ready.is_empty()
        {
            return;
        }
        out.push_str("<scroll id='asset-import-scroll' style='max-height:180px;min-height:0px;flex-shrink:0'><panel id='asset-import-jobs' style='padding:6px;gap:6px;flex-direction:column'>");
        let queued = self.asset_import.queued.len() + self.asset_import.ready.len();
        if queued > 0 {
            out.push_str(&format!(
                "<text id='asset-import-queued' class='caption' text='{} import batch{} queued' />",
                queued,
                if queued == 1 { "" } else { "es" }
            ));
        }
        for index in 0..self.asset_import.jobs.len() {
            let job = &self.asset_import.jobs[index];
            let key = format!("asset-import-{:016x}", ara_ui::source::stable_id(&job.id));
            let id = job.id.clone();
            let running = job.status.running();
            let can_cancel = job.status.can_cancel() && !job.cancel_requested;
            let summary = if let Some(error) = &job.error {
                error.clone()
            } else if running && job.cancel_requested {
                "Cancelling import…".into()
            } else if job.preparation.is_some() && running {
                format!("Checking destinations · {}", job.status.message)
            } else if running {
                job.status.message.clone()
            } else {
                job.status.summary()
            };
            let destination = if job.destination.is_empty() {
                "Project content"
            } else {
                &job.destination
            };
            out.push_str(&format!("<panel id='{key}' class='notice' style='flex-direction:column;gap:4px;min-width:0px'><panel id='{key}-heading' class='row' style='gap:6px'><text id='{key}-title' class='fill' style='white-space:normal;min-width:0px' text='{} file{} → {}' />",
                job.sources,if job.sources==1 {""} else {"s"},escape(destination)));
            let (command, label) = if running {
                ("assets.import.cancel", "Cancel")
            } else {
                ("assets.import.dismiss", "Dismiss")
            };
            out.push_str(&format!("<button id='{key}-action' class='ghost' action='Dispatch' label='{label}'{}><text id='{key}-action-label' text='{label}' /></button></panel>",
                if running&&!can_cancel {" disabled='true' description='Finishing the import transaction; cancellation is no longer available'"} else {""}));
            out.push_str(&format!(
                "<text id='{key}-message' class='caption' style='white-space:normal' text='{}' />",
                escape(&summary)
            ));
            if running {
                out.push_str(&format!("<panel id='{key}-progress' class='progress' style='height:3px'><panel id='{key}-fill' class='progress-fill' style='height:3px;width:{}%' /></panel>",job.status.progress*100.));
            }
            out.push_str("</panel>");
            self.bind_command(&format!("{key}-action"), command, Target::Text(id));
        }
        out.push_str("</panel></scroll>");
    }
}
