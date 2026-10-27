use super::*;
impl Shell {
    pub(super) fn file_drag_target(&self, editor: &Editor, batch: u64) -> Feedback<Folder> {
        let reject = |reason: &str, indicator| Feedback::Reject {
            reason: reason.into(),
            indicator,
        };
        if self.modal_active() || self.choice_open() || self.content_pending() || self.tab_pending()
        {
            return reject("Finish the current interaction before dropping files", None);
        }
        let [x, y] = self.pointer;
        for &index in self.ui.paint_order.iter().rev() {
            let node = &self.ui.nodes[index];
            if !self.ui.shown(index) || !node.rect.contains(x, y) || !node.clip.contains(x, y) {
                continue;
            }
            let storage = match self
                .bindings
                .get(&ara_ui::source::stable_id(&node.element.id))
            {
                Some(Binding::AssetFolder(folder, false)) => Some(folder.as_str()),
                Some(Binding::Asset(asset)) if asset.kind == "folder" => Some(asset.path.as_str()),
                Some(Binding::Protocol("editor.workspace", params)) => {
                    params.get("folder").and_then(Value::as_str)
                }
                _ => None,
            };
            if let Some(storage) = storage {
                return self.file_drag_folder(
                    batch,
                    storage,
                    Indicator::Inside(node.rect.intersection(node.clip)),
                );
            }
        }
        for id in ["asset-grid", "asset-rows", "assets-empty"] {
            if let Some(index) = self.shown_node(id) {
                let rect = self.ui.nodes[index]
                    .rect
                    .intersection(self.ui.nodes[index].clip);
                if rect.contains(x, y) {
                    return self.file_drag_folder(
                        batch,
                        &editor.state.asset_folder,
                        Indicator::Inside(rect),
                    );
                }
            }
        }
        reject("Drop files onto a folder in Assets", None)
    }
    fn file_drag_folder(
        &self,
        batch: u64,
        storage: &str,
        indicator: Indicator,
    ) -> Feedback<Folder> {
        match self.browser.authored_folder(storage) {
            Ok(authored) => {
                let label = if authored.is_empty() {
                    "Import into authored content".into()
                } else {
                    format!("Import into {}", names::file_name(storage))
                };
                Feedback::Accept {
                    target: Folder {
                        batch,
                        storage: storage.into(),
                        authored,
                    },
                    indicator,
                    label,
                }
            }
            Err(reason) => Feedback::Reject {
                reason,
                indicator: Some(indicator),
            },
        }
    }
}
