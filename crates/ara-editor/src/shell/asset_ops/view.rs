//! Shared kit fields, dialogs and command-bound progress controls.
use super::*;
struct NoClipboard;
impl ara_ui_text::Clipboard for NoClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
impl Shell {
    pub(crate) fn asset_file_name_markup(&mut self, out: &mut String) {
        let Some(rename) = &self.asset_ops.rename else {
            return;
        };
        self.bindings.insert(
            ara_ui::source::stable_id("asset-file-name"),
            Binding::AssetFile(Action::RenameValue),
        );
        out.push_str(&format!("<field id='asset-file-name' style='position:absolute;left:0px;top:0px;width:160px;height:24px' text='{}' label='Asset name' description='Enter renames the asset; Escape keeps the current name. Include the extension.' action='Dispatch' />",escape(&rename.name)));
    }
    pub(crate) fn position_asset_file_name(&mut self) -> ara_ui::Result<()> {
        let Some(rename) = &self.asset_ops.rename else {
            return Ok(());
        };
        let Some(field) = self.ui.find("asset-file-name") else {
            return Ok(());
        };
        let source = &rename.asset.path;
        let node=self.ui.nodes.iter().enumerate().find(|(index,node)|matches!(self.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::Asset(asset)) if &asset.path==source) && self.ui.shown(*index) && node.rect.intersection(node.clip).w>0. && node.rect.intersection(node.clip).h>0.).map(|(_,node)|node);
        let rect = node.and_then(|node| {
            self.ui
                .find(&format!("{}-label", node.element.id))
                .map(|label| self.ui.nodes[label].rect)
                .or(Some(node.rect))
        });
        let Some(rect) = rect else {
            self.ui.set_visible(field, false)?;
            return Ok(());
        };
        let width = rect.w.clamp(96., 360.).min(self.ui.size[0].max(1.));
        self.ui.set_visible(field, true)?;
        if (self.ui.nodes[field].rect.w - width).abs() > 0.25 {
            self.ui.set_style(
                field,
                &format!("position:absolute;left:0px;top:0px;width:{width}px;height:24px"),
            )?;
        }
        self.ui.nodes[field].paint_offset = [
            rect.x.clamp(0., (self.ui.size[0] - width).max(0.)),
            (rect.y - 3.).clamp(0., (self.ui.size[1] - 24.).max(0.)),
        ];
        self.ui.nodes[field].dirty.paint = true;
        if rename.focus_requested {
            let end = if rename.asset.kind == "folder" {
                rename.name.len()
            } else {
                rename
                    .name
                    .rsplit_once('.')
                    .map_or(rename.name.len(), |(stem, _)| stem.len())
            };
            self.input
                .focus(&mut self.ui, ara_ui::source::stable_id("asset-file-name"))?;
            self.ui.edit(
                field,
                ara_ui_text::EditEvent::Select {
                    anchor: 0,
                    caret: end,
                },
                &mut NoClipboard,
            )?;
            if let Some(rename) = &mut self.asset_ops.rename {
                rename.focus_requested = false;
            }
        }
        Ok(())
    }
    pub(in crate::shell) fn asset_operations_markup(&mut self, out: &mut String) {
        if let Some(rename) = &self.asset_ops.rename
            && let Some(error) = &rename.error
        {
            out.push_str(&format!(
                "<text id='asset-rename-error' class='caption asset-job-message' text='{}' />",
                escape(error)
            ));
        }
        let Some(job) = &self.asset_ops.job else {
            return;
        };
        let progress = job.status["progress"]
            .as_f64()
            .filter(|value| value.is_finite())
            .unwrap_or(0.)
            .clamp(0., 1.);
        let pending = backend::running(&job.status);
        let message = if job.following {
            "Transaction complete · refreshing authored entries".into()
        } else {
            backend::message(&job.status, &job.plan.labels)
        };
        let cancel = backend::can_cancel(&job.status) && !job.cancel_requested;
        out.push_str(&format!("<panel id='asset-operation' class='asset-job'><panel id='asset-operation-title-row' class='row' style='gap:6px;min-width:0px;flex-shrink:0'><text id='asset-operation-title' class='caption asset-job-title' text='{}{}' />",escape(&job.plan.title),if pending{format!(" · {:.0}%",progress*100.)}else{String::new()}));
        if pending {
            self.bind_command(
                "asset-operation-cancel",
                "assets.operation.cancel",
                commands::Target::None,
            );
            out.push_str(&format!("<button id='asset-operation-cancel' class='ghost' label='Cancel preparation' {} action='Dispatch'><text id='asset-operation-cancel-label' text='Cancel' /></button>",if cancel{""}else{"disabled='true' description='Cancellation is unavailable at this stage'"}));
        } else if !self.asset_ops.busy() {
            self.bind_command(
                "asset-operation-dismiss",
                "assets.operation.dismiss",
                commands::Target::None,
            );
            out.push_str("<button id='asset-operation-dismiss' class='ghost icon-button' label='Dismiss asset result' action='Dispatch'><text id='asset-operation-dismiss-label' text='×' /></button>");
        }
        out.push_str(&format!("</panel><text id='asset-operation-message' class='caption asset-job-message' text='{}' /></panel>",escape(&message)));
    }
    pub(crate) fn asset_operation_report(&self) -> Value {
        json!({"busy":self.asset_ops.busy(),"followingHistory":self.asset_ops.history.pending(),"renaming":self.asset_ops.rename.as_ref().map(|rename|json!({"path":rename.asset.path,"authoredPath":rename.asset.authored_path,"name":rename.name,"error":rename.error})),"job":self.asset_ops.job.as_ref().map(|job|json!({"id":job.id,"status":job.status,"labels":job.plan.labels,"followingIndex":job.following,"cancelRequested":job.cancel_requested,"preservedSelection":job.selection.paths})),"prompt":self.asset_ops.prompt.is_some(),"confirmation":self.asset_ops.confirmation.is_some()})
    }

    #[cfg(test)]
    pub(in crate::shell) fn seed_asset_operation_rail(&mut self, editor: &Editor, running: bool) {
        // Presentation-only receipt; tests click and inspect the retained intent,
        // while native acceptance owns real worker cancellation and completion.
        self.asset_ops.job = Some(Job {
            id: "jb_operationsrail".into(),
            status: if running {
                json!({"status":"running","cancellable":true,"progress":0.5,"message":"Checking asset references before committing the selected file operation"})
            } else {
                json!({"status":"done","message":"Asset operation complete; the selected files remain available in the browser"})
            },
            polled: Instant::now(),
            began: Instant::now(),
            plan: plan::Plan::new(
                "Move selected assets",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                false,
            ),
            selection: Selection::read(self, editor),
            following: false,
            scan_barrier: None,
            scan_wait_notice: false,
            cancel_requested: false,
        });
        self.asset_ops.revision += 1;
    }
}
