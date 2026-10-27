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
fn reference(snapshot: &Value, path: &str) -> Option<String> {
    Some(format!(
        "@{path}#{}",
        snapshot["documents"][path]["id"].as_str()?
    ))
}
fn has_reference(snapshot: &Value, value: &str) -> bool {
    snapshot["references"]
        .as_array()
        .is_some_and(|references| references.iter().any(|reference| reference == value))
}
fn moved_document(before: &Value, after: &Value, source: &str, destination: &str) -> bool {
    before["documents"][source].is_object()
        && after["documents"][source].is_null()
        && after["documents"][destination]["id"] == before["documents"][source]["id"]
        && reference(before, source)
            .zip(reference(after, destination))
            .is_some_and(|(old, new)| !has_reference(after, &old) && has_reference(after, &new))
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
        shell: &Shell,
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
        let content_ok = match act {
            Act::Rename => {
                moved_document(
                    &self.before,
                    &after,
                    &act.source(),
                    &format!("{}/Renamed.prefab", act.path()),
                ) && self.before["files"]["A.prefab"] == after["files"]["Renamed.prefab"]
            }
            Act::Move => ["A.prefab", "B.prefab"].into_iter().all(|file| {
                moved_document(
                    &self.before,
                    &after,
                    &format!("{}/{file}", act.path()),
                    &format!("{}/Destination/{file}", act.path()),
                ) && self.before["files"][file] == after["files"][format!("Destination/{file}")]
            }),
            Act::Duplicate => {
                let ids: Option<Vec<_>> = ["A Copy.prefab", "B Copy.prefab"]
                    .into_iter()
                    .map(|file| after["documents"][format!("{}/{file}", act.path())]["id"].as_str())
                    .collect();
                ids.is_some_and(|ids| {
                    ids.iter().copied().collect::<BTreeSet<_>>().len() == 2
                        && ids.iter().all(|id| {
                            self.before["documents"]
                                .as_object()
                                .is_some_and(|documents| {
                                    documents.values().all(|doc| doc["id"] != *id)
                                })
                        })
                }) && self.before["documents"]
                    .as_object()
                    .is_some_and(|documents| {
                        documents
                            .iter()
                            .all(|(path, doc)| &after["documents"][path] == doc)
                    })
            }
            Act::Delete => {
                after["files"]["A.prefab"].is_null()
                    && after["files"]["B.prefab"].is_null()
                    && after["documents"][act.source()].is_null()
                    && after["documents"][format!("{}/B.prefab", act.path())].is_null()
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
        let ok = content_ok
            && transactions == expected_transactions
            && (!act.undo()
                || self.undo.as_ref() == Some(&self.before)
                    && self.committed.as_ref() == Some(&after));
        self.results.push(json!({"journey":act.label(),"ok":ok,"transactions":transactions,"expectedTransactions":expected_transactions,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),"before":self.before,"after":after,"terminal":self.terminal,"selectionBefore":self.selection,"selectionBeforePaths":self.selection_paths,"selectionCommitted":self.committed_selection,"selectionUndo":self.undo_selection,"selectionRedo":self.redo_selection,"laterHumanSelection":self.unrelated_selection,"selectionAfter":fixture::selection(editor,shell),"dragPreview":self.drag_preview,"cancelablePreparation":self.cancel_observed,"indexProjection":self.index_preview,"trace":self.trace,"events":self.events}));
        self.advance();
        self.write()
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
        self.trace.push(json!({"stage":stage,"age":self.age,"revision":editor.host.store().revision(),"assetOperation":shell.asset_operation_report(),"drag":shell.content_drag_report(),"selection":fixture::selection(editor,shell),"forceButtons":force_buttons(shell)}));
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
