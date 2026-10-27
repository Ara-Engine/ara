//! Drain each proof's own asynchronous work through ordinary native Cancel/Dismiss controls.
use super::*;

impl ImportCheck {
    pub(super) fn recover(
        &mut self,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        if self.started.elapsed() > Duration::from_secs(30) {
            return Err("Native import cleanup could not drain its jobs and close its mounted dialogs; refusing to contaminate the next proof".into());
        }
        let report = shell.asset_import_report();
        if report["choosingConflict"] == true || shell.modal_active() {
            let mounted = shell.ui.nodes.iter().any(|node| {
                node.element.tag == "modal" && shell.shown_node(&node.element.id).is_some()
            });
            if mounted {
                key(window, 41, events)?;
            }
            return Ok(false);
        }
        if report["queued"].as_u64().unwrap_or(0) > 0 || report["ready"].as_u64().unwrap_or(0) > 0 {
            return Ok(false);
        }
        for job in report["jobs"].as_array().into_iter().flatten() {
            let Some(id) = job["job"]
                .as_str()
                .filter(|id| !self.old_jobs.contains(*id))
            else {
                continue;
            };
            let running = job["record"]["status"] == "running";
            if running && !cancellable(&job["record"]) {
                // A live false/missing flag is authoritative: wait for the receipt;
                // never try to force cancellation across the commit boundary.
                return Ok(false);
            }
            click(
                shell,
                &format!("asset-import-{:016x}-action", ara_ui::source::stable_id(id)),
                window,
                scale,
                events,
            )?;
            return Ok(false);
        }
        if !shell.file_drag_report().is_null() {
            native_input::focus_lost(window, events)?;
            return Ok(false);
        }
        Ok(!shell.choice_open() && shell.input.captured().is_none())
    }
}
