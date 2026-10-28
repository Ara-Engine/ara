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
            if let Some(result) = self.results.last_mut() {
                result["cleanupFailure"] = json!({"error":"Timed out waiting for mounted native import controls","nativeEvents":self.cleanup_events});
            }
            self.write()?;
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
            let key = format!("asset-import-{:016x}", ara_ui::source::stable_id(id));
            let action = format!("{key}-action");
            let title = format!("{key}-title");
            let probe = control_probe(shell, &key);
            if let Some(result) = self.results.last_mut() {
                result["cleanupLastObserved"] = json!({"job":id,"status":job["record"]["status"],"cancellable":job["record"]["cancellable"],"controls":probe});
            }
            let expected = if running { "Cancel" } else { "Dismiss" };
            if probe["label"]["text"] != expected {
                return Ok(false);
            }
            if probe["action"]["fullyVisible"] == true && probe["title"]["fullyVisible"] != true {
                if let (Some(target), Some(scroll)) = (
                    shell.shown_node(&title),
                    shell.shown_node("asset-jobs-scroll"),
                ) {
                    let target = &shell.ui.nodes[target];
                    let viewport = shell.ui.nodes[scroll]
                        .rect
                        .intersection(shell.ui.nodes[scroll].clip);
                    if viewport.w > 2. && viewport.h > 2. {
                        native_input::motion(
                            window,
                            [viewport.x + viewport.w * 0.5, viewport.y + viewport.h * 0.5],
                            scale,
                            events,
                        )?;
                        native_input::wheel(
                            window,
                            if target.rect.y < viewport.y {
                                0.125
                            } else {
                                -0.125
                            },
                            events,
                        )?;
                    }
                }
                return Ok(false);
            }
            if !self.cleanup_controls_captured
                && probe["action"]["fullyVisible"] == true
                && probe["title"]["fullyVisible"] == true
            {
                // Capture the real title and actionable card after native scrolling,
                // before its native Cancel/Dismiss click removes the evidence.
                self.cleanup_capture_pending = true;
                return Ok(false);
            }
            click(shell, &action, window, scale, events)?;
            return Ok(false);
        }
        if !shell.file_drag_report().is_null() {
            native_input::focus_lost(window, events)?;
            return Ok(false);
        }
        Ok(!shell.choice_open() && shell.input.captured().is_none())
    }
}

fn control_probe(shell: &Shell, key: &str) -> Value {
    let node = |id: &str| {
        shell.shown_node(id).map_or(Value::Null,|index| {
            let node=&shell.ui.nodes[index];
            let visible=node.rect.intersection(node.clip);
            json!({"id":id,"rect":node.rect,"clip":node.clip,"disabled":node.state.disabled,
                "fullyVisible":node.rect.w>2. && node.rect.h>2. && visible.w>=node.rect.w*0.99 && visible.h>=node.rect.h*0.99,
                "text":node.text.as_ref().map(|text|text.value.as_str())})
        })
    };
    json!({"viewport":node("asset-jobs-scroll"),"title":node(&format!("{key}-title")),
        "action":node(&format!("{key}-action")),"label":node(&format!("{key}-action-label"))})
}
