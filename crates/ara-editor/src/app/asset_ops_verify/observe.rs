//! Read-only authority/file/index observations and completed native-target captures.
use super::*;
use crate::shell::Binding;
/// Resolve every selected storage key through the actual cached semantic index.
pub(super) fn selection_paths(shell: &Shell, selection: &Value) -> ara_ui::Result<Value> {
    let keys = selection["browserKeys"]
        .as_array()
        .ok_or("Asset selection omitted its storage keys")?;
    let mut paths = BTreeMap::new();
    for key in keys {
        let key = key
            .as_u64()
            .ok_or("Asset selection key was not an integer")?;
        let file = shell
            .asset_authored_files()
            .iter()
            .find(|file| ara_ui::source::stable_id(&file.storage_path) == key)
            .ok_or("Selected asset key is stale or missing from the cached semantic index")?;
        paths.insert(
            file.storage_path.clone(),
            json!({"path":file.path,"authored":matches!(&file.scope,AuthoredScope::Authored)}),
        );
    }
    if selection["asset"] != selection["browserPrimary"] {
        return Err("Browser primary and inspected asset diverged".into());
    }
    if let Some(primary) = selection["asset"].as_str()
        && !paths.contains_key(primary)
    {
        return Err("Inspected asset was not in the browser multi-selection".into());
    }
    Ok(json!(paths))
}
pub(super) fn projection(shell: &Shell) -> Value {
    let files:Vec<_>=shell.asset_authored_files().iter().map(|file|json!({"path":file.path,"storagePath":file.storage_path,"authored":matches!(&file.scope,AuthoredScope::Authored),"folder":matches!(&file.role,AuthoredRole::Folder)})).collect();
    let visible: BTreeSet<_> = shell
        .ui
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            if !shell.ui.shown(index)
                || node.rect.intersection(node.clip).w <= 0.
                || node.rect.intersection(node.clip).h <= 0.
            {
                return None;
            }
            match shell
                .bindings
                .get(&ara_ui::source::stable_id(&node.element.id))
            {
                Some(Binding::Asset(asset)) => Some(asset.path.clone()),
                _ => None,
            }
        })
        .collect();
    json!({"files":files,"visibleAssets":visible})
}
fn force_buttons(shell: &Shell) -> Vec<String> {
    shell
        .ui
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            if node.element.tag != "button" || !shell.ui.shown(index) {
                return None;
            }
            let title = node
                .element
                .attrs
                .get("label")
                .or_else(|| node.element.attrs.get("text"))?;
            title
                .to_ascii_lowercase()
                .contains("force")
                .then(|| title.clone())
        })
        .collect()
}
/// Snapshots retain storage keys; command/reference inputs remain authored-relative.
fn document<'a>(snapshot: &'a Value, workspace: &ara_doc::Workspace, path: &str) -> &'a Value {
    &snapshot["documents"][workspace.storage_path(path)]
}
fn reference(snapshot: &Value, workspace: &ara_doc::Workspace, path: &str) -> Option<String> {
    Some(format!(
        "@{path}#{}",
        document(snapshot, workspace, path)["id"].as_str()?
    ))
}
fn has_reference(snapshot: &Value, value: &str) -> bool {
    snapshot["references"]
        .as_array()
        .is_some_and(|references| references.iter().any(|reference| reference == value))
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MovedDocument {
    source_storage_path: String,
    destination_storage_path: String,
    source_present: bool,
    source_removed: bool,
    id_preserved: bool,
    old_reference_removed: bool,
    new_reference_present: bool,
}
impl MovedDocument {
    fn ok(&self) -> bool {
        self.source_present
            && self.source_removed
            && self.id_preserved
            && self.old_reference_removed
            && self.new_reference_present
    }
}
fn moved_document(
    before: &Value,
    after: &Value,
    workspace: &ara_doc::Workspace,
    source: &str,
    destination: &str,
) -> MovedDocument {
    MovedDocument {
        source_storage_path: workspace.storage_path(source),
        destination_storage_path: workspace.storage_path(destination),
        source_present: document(before, workspace, source).is_object(),
        source_removed: document(after, workspace, source).is_null(),
        id_preserved: document(before, workspace, source)["id"]
            .as_str()
            .zip(document(after, workspace, destination)["id"].as_str())
            .is_some_and(|(old, new)| old == new),
        old_reference_removed: reference(before, workspace, source)
            .is_some_and(|old| !has_reference(after, &old)),
        new_reference_present: reference(after, workspace, destination)
            .is_some_and(|new| has_reference(after, &new)),
    }
}
fn diagnostic_index(status: &Value, index: u64) -> bool {
    status["diagnostics"].as_array().is_some_and(|problems| {
        problems.iter().any(|problem| {
            problem["location"]["command"]
                .as_u64()
                .or_else(|| problem["command"].as_u64())
                == Some(index)
        })
    })
}
impl AssetOpsCheck {
    pub(super) fn outcome(
        &mut self,
        act: Act,
        editor: &Editor,
        shell: &Shell,
        storage: &str,
    ) -> ara_ui::Result<()> {
        let report = shell.asset_operation_report();
        let job = &report["job"];
        if act.no_job() {
            if report["confirmation"] == true
                || report["renaming"].is_object()
                || shell.content_pending()
            {
                return self.wait();
            }
            let current = job["id"].as_str();
            if current != self.old_job.as_deref() {
                return Err(
                    "A cancelled/invalid native gesture unexpectedly started an asset transaction"
                        .into(),
                );
            }
            self.terminal = Some(Value::Null);
        } else {
            let Some(id) = job["id"]
                .as_str()
                .filter(|id| Some(*id) != self.old_job.as_deref())
            else {
                return self.wait();
            };
            if job["status"]["status"] == "running" {
                if fixture::selection(editor, shell) != self.selection {
                    return Err(
                        "Selection changed while the asset transaction was still preparing".into(),
                    );
                }
                return self.wait();
            }
            let expected = if act.fails() {
                "failed"
            } else if act == Act::CancelPreparation {
                "cancelled"
            } else {
                "done"
            };
            if job["status"]["status"] != expected {
                return Err(format!(
                    "Native asset job {id} finished unexpectedly: {}",
                    ara_json::to_canonical_string(job)
                )
                .into());
            }
            if job["followingIndex"] == true || report["followingHistory"] == true {
                return self.wait();
            }
            self.terminal = Some(job.clone());
        }
        if editor.host.unsaved() {
            return self.wait();
        }
        let selection = fixture::selection(editor, shell);
        let paths = selection_paths(shell, &selection)?;
        if act.undo() && !job["status"]["result"]["txn"].is_string() {
            return Err(
                "The completed asset batch omitted its transaction ID needed for history selection"
                    .into(),
            );
        }
        if act.undo() {
            let expected: BTreeSet<_> = match act {
                Act::Rename => vec![format!("{}/Renamed.prefab", act.path())],
                Act::Move => ["A.prefab", "B.prefab"]
                    .into_iter()
                    .map(|file| format!("{}/Destination/{file}", act.path()))
                    .collect(),
                Act::Duplicate => ["A Copy.prefab", "B Copy.prefab"]
                    .into_iter()
                    .map(|file| format!("{}/{file}", act.path()))
                    .collect(),
                Act::Folder => vec![format!("{}/Created natively", act.path())],
                _ => Vec::new(),
            }
            .into_iter()
            .collect();
            let selected: BTreeSet<_> = paths
                .as_object()
                .into_iter()
                .flat_map(|paths| paths.values())
                .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
                .collect();
            if selected != expected {
                return Err(
                    "Completed native asset gesture did not select the resulting authored paths"
                        .into(),
                );
            }
        }
        self.committed_selection = Some(selection);
        self.committed = Some(fixture::snapshot(editor, storage)?);
        Ok(())
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed_frame) || self.finished() || self.waiting {
            return Ok(());
        }
        let act = ACTS[self.index];
        if let Some(stage) = match self.age {
            0 => Some("before"),
            4 => Some("menu-or-edit"),
            5 => Some("preview"),
            7 => Some("committed-or-blocked"),
            10 if act.undo() => Some("undo"),
            12 if act.undo() => Some("redo"),
            15 if act == Act::Duplicate => Some("human-selection-undo"),
            17 if act == Act::Duplicate => Some("human-selection-redo"),
            FINISH => Some("after"),
            _ => None,
        } {
            self.capture(editor, shell, gpu, act, stage)?;
        }
        if self.age < FINISH {
            self.age += 1;
            return Ok(());
        }
        let storage =
            fixture::storage(shell, &act.path()).ok_or("Asset proof folder disappeared")?;
        let after = fixture::snapshot(editor, &storage)?;
        let transactions = editor
            .host
            .store()
            .history()
            .len()
            .saturating_sub(self.before_history);
        let terminal = self.terminal.as_ref().unwrap_or(&Value::Null);
        let unchanged = after == self.before;
        let expected_transactions = if act == Act::Duplicate {
            5
        } else if act.undo() {
            3
        } else {
            0
        };
        let workspace = editor.host.store().workspace();
        let mut content_checks = Value::Null;
        let content_ok = match act {
            Act::Rename => {
                let moved = moved_document(
                    &self.before,
                    &after,
                    workspace,
                    &act.source(),
                    &format!("{}/Renamed.prefab", act.path()),
                );
                let bytes_preserved =
                    self.before["files"]["A.prefab"] == after["files"]["Renamed.prefab"];
                content_checks = json!({"document":moved,"fileBytesPreserved":bytes_preserved});
                moved.ok() && bytes_preserved
            }
            Act::Move => {
                let mut checks = Vec::new();
                let mut valid = true;
                for file in ["A.prefab", "B.prefab"] {
                    let moved = moved_document(
                        &self.before,
                        &after,
                        workspace,
                        &format!("{}/{file}", act.path()),
                        &format!("{}/Destination/{file}", act.path()),
                    );
                    let bytes_preserved =
                        self.before["files"][file] == after["files"][format!("Destination/{file}")];
                    valid &= moved.ok() && bytes_preserved;
                    checks.push(
                        json!({"file":file,"document":moved,"fileBytesPreserved":bytes_preserved}),
                    );
                }
                content_checks = json!(checks);
                valid
            }
            Act::Duplicate => {
                let ids: Option<Vec<_>> = ["A Copy.prefab", "B Copy.prefab"]
                    .into_iter()
                    .map(|file| {
                        document(&after, workspace, &format!("{}/{file}", act.path()))["id"]
                            .as_str()
                    })
                    .collect();
                let fresh_ids = ids.is_some_and(|ids| {
                    ids.iter().copied().collect::<BTreeSet<_>>().len() == 2
                        && ids.iter().all(|id| {
                            self.before["documents"]
                                .as_object()
                                .is_some_and(|documents| {
                                    documents.values().all(|doc| doc["id"] != *id)
                                })
                        })
                });
                let originals_unchanged =
                    self.before["documents"]
                        .as_object()
                        .is_some_and(|documents| {
                            documents
                                .iter()
                                .all(|(path, doc)| &after["documents"][path] == doc)
                        });
                content_checks = json!({"copyIdsFreshAndDistinct":fresh_ids,"originalDocumentsUnchanged":originals_unchanged});
                fresh_ids && originals_unchanged
            }
            Act::Delete => {
                let files_removed =
                    after["files"]["A.prefab"].is_null() && after["files"]["B.prefab"].is_null();
                let source_present = document(&self.before, workspace, &act.source()).is_object()
                    && document(&self.before, workspace, &format!("{}/B.prefab", act.path()))
                        .is_object();
                let documents_removed = document(&after, workspace, &act.source()).is_null()
                    && document(&after, workspace, &format!("{}/B.prefab", act.path())).is_null();
                content_checks = json!({"filesRemoved":files_removed,"sourceDocumentsPresent":source_present,"documentsRemoved":documents_removed});
                files_removed && source_present && documents_removed
            }
            Act::Folder => after["directories"].as_array().is_some_and(|directories| {
                directories
                    .iter()
                    .any(|directory| directory == "Created natively")
            }),
            Act::RenameCancel | Act::DeleteCancel => {
                unchanged && fixture::selection(editor, shell) == self.selection
            }
            Act::InvalidFolder => {
                unchanged
                    && fixture::selection(editor, shell) == self.selection
                    && self.drag_preview["feedback"]["valid"] == false
            }
            Act::MoveConflict | Act::SafeDelete => {
                unchanged
                    && fixture::selection(editor, shell) == self.selection
                    && terminal["status"]["status"] == "failed"
                    && diagnostic_index(&terminal["status"], 1)
                    && self
                        .trace
                        .iter()
                        .all(|trace| trace["forceButtons"].as_array().is_none_or(Vec::is_empty))
            }
            Act::CancelPreparation => {
                unchanged
                    && fixture::selection(editor, shell) == self.selection
                    && self.cancel_observed.is_some()
                    && terminal["status"]["status"] == "cancelled"
            }
            Act::Index => {
                index_ok(&self.index_preview)
                    && editor.state.asset_folder == "settings/builds"
                    && unchanged
            }
        };
        let transactions_ok = transactions == expected_transactions;
        let undo_ok = !act.undo() || self.undo.as_ref() == Some(&self.before);
        let redo_ok = !act.undo() || self.committed.as_ref() == Some(&after);
        let ok = content_ok && transactions_ok && undo_ok && redo_ok;
        self.results.push(json!({"journey":act.label(),"ok":ok,"checks":{"content":content_ok,"transactions":transactions_ok,"undoRestoredExactSnapshot":undo_ok,"redoRestoredExactSnapshot":redo_ok,"contentPredicates":content_checks},"transactions":transactions,"expectedTransactions":expected_transactions,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),"before":self.before,"after":after,"terminal":self.terminal,"selectionBefore":self.selection,"selectionBeforePaths":self.selection_paths,"selectionCommitted":self.committed_selection,"selectionUndo":self.undo_selection,"selectionRedo":self.redo_selection,"laterHumanSelection":self.unrelated_selection,"selectionAfter":fixture::selection(editor,shell),"dragPreview":self.drag_preview,"cancelablePreparation":self.cancel_observed,"indexProjection":self.index_preview,"trace":self.trace,"events":self.events}));
        self.advance(shell);
        self.write()?;
        if !ok {
            return Err(format!(
                "Native asset story {} failed its checks; inspect asset-operations.json",
                act.label()
            )
            .into());
        }
        Ok(())
    }
    fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        act: Act,
        stage: &str,
    ) -> ara_ui::Result<()> {
        let prefix = format!("assets-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{prefix}.png")))?;
        std::fs::write(
            self.output.join(format!("{prefix}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        self.trace.push(json!({"stage":stage,"age":self.age,"revision":editor.host.store().revision(),"assetOperation":shell.asset_operation_report(),"historyReceipt":self.history_probe,"focus":shell.input.focused().and_then(|id|shell.ui.by_id(id)).map(|index|shell.ui.nodes[index].element.id.as_str()),"drag":shell.content_drag_report(),"selection":fixture::selection(editor,shell),"forceButtons":force_buttons(shell)}));
        Ok(())
    }
}
fn index_ok(preview: &Value) -> bool {
    let Some(files) = preview["files"].as_array() else {
        return false;
    };
    let storage: Vec<_> = files
        .iter()
        .filter_map(|file| file["storagePath"].as_str())
        .collect();
    let authored: Vec<_> = files
        .iter()
        .filter(|file| file["authored"] == true)
        .filter_map(|file| file["path"].as_str())
        .collect();
    let required = [
        format!("{ROOT}/Index/packages/Valid/Nested.prefab"),
        format!("{ROOT}/Index/target/builds/Nested.prefab"),
        format!("{ROOT}/Index/settings/builds/Content.prefab"),
    ];
    required.iter().all(|path| {
        authored.contains(&path.as_str())
            && files
                .iter()
                .find(|file| file["path"] == *path && file["authored"] == true)
                .and_then(|file| file["storagePath"].as_str())
                .is_some_and(|path| {
                    preview["visibleAssets"]
                        .as_array()
                        .is_some_and(|visible| visible.iter().any(|shown| shown == path))
                })
    }) && files.iter().any(|file| {
        file["storagePath"] == "settings/builds/p4-native.export.json" && file["authored"] == false
    }) && storage.iter().all(|path| {
        !matches!(*path, ".ara" | "packages" | "builds")
            && !path.starts_with(".ara/")
            && !path.starts_with("packages/")
            && !path.starts_with("builds/")
    })
}

#[cfg(test)]
mod tests;
