//! Poll completed authority work; selection follows only a successful cached index handoff.
use super::*;
enum Desired {
    Storage(String),
    Authored(String),
}
fn relocated(path: &str, changes: &[(assets::Asset, Option<String>)]) -> Option<Desired> {
    for (source, destination) in changes {
        let tail = if path == source.path {
            Some("")
        } else if source.kind == "folder" {
            path.strip_prefix(&source.path)
                .and_then(|tail| tail.strip_prefix('/'))
        } else {
            None
        };
        if let Some(tail) = tail {
            return destination.as_ref().map(|destination| {
                Desired::Authored(if tail.is_empty() {
                    destination.clone()
                } else {
                    plan::join(destination, tail)
                })
            });
        }
    }
    Some(Desired::Storage(path.into()))
}
fn resolve(path: Desired, items: &[assets::Asset]) -> Option<String> {
    match path {
        Desired::Storage(path) => Some(path),
        Desired::Authored(path) => items
            .iter()
            .find(|asset| asset.authored_path.as_deref() == Some(path.as_str()))
            .map(|asset| asset.path.clone()),
    }
}
/// Resolve a future row from its cached authored parent. The workspace's authoring
/// root handles a root-level destination; project-scope folders never participate.
fn projected(
    workspace: &ara_doc::Workspace,
    path: Desired,
    items: &[assets::Asset],
) -> Option<String> {
    match path {
        Desired::Storage(path) => Some(path),
        Desired::Authored(path) => {
            if let Some(asset) = items
                .iter()
                .find(|asset| asset.authored_path.as_deref() == Some(path.as_str()))
            {
                return Some(asset.path.clone());
            }
            let parent = items
                .iter()
                .filter(|asset| asset.kind == "folder")
                .filter_map(|asset| {
                    let logical = asset.authored_path.as_deref()?;
                    let suffix = path.strip_prefix(logical)?.strip_prefix('/')?;
                    Some((logical.len(), asset.path.as_str(), suffix))
                })
                .max_by_key(|(length, _, _)| *length);
            Some(parent.map_or_else(
                || workspace.storage_path(&path),
                |(_, storage, suffix)| plan::join(storage, suffix),
            ))
        }
    }
}
fn planned_selection(
    before: &Selection,
    plan: &plan::Plan,
    items: &[assets::Asset],
    resolve: impl Fn(Desired, &[assets::Asset]) -> Option<String>,
) -> Option<Selection> {
    let desired: Vec<_> = if plan.created.is_empty() {
        before
            .paths
            .iter()
            .filter_map(|path| relocated(path, &plan.changes))
            .collect()
    } else {
        plan.created
            .iter()
            .cloned()
            .map(Desired::Authored)
            .collect()
    };
    let mut paths: Vec<_> = desired
        .into_iter()
        .map(|path| resolve(path, items))
        .collect::<Option<_>>()?;
    let folder = if plan.created.is_empty() {
        relocated(&before.folder, &plan.changes)
            .unwrap_or_else(|| Desired::Storage(plan::parent(&before.folder).into()))
    } else {
        Desired::Storage(before.folder.clone())
    };
    let folder = resolve(folder, items)?;
    let asset = if plan.created.is_empty() {
        before
            .asset
            .as_deref()
            .and_then(|path| relocated(path, &plan.changes))
            .and_then(|path| resolve(path, items))
            .filter(|path| paths.contains(path))
            .or_else(|| paths.first().cloned())
    } else {
        paths.first().cloned()
    };
    paths.sort();
    paths.dedup();
    let mut keys: Vec<_> = paths
        .iter()
        .map(|path| ara_ui::source::stable_id(path))
        .collect();
    keys.sort_unstable();
    Some(Selection {
        doc: before.doc.clone(),
        entities: before.entities.clone(),
        primary: asset.clone(),
        active: asset.as_deref().map(ara_ui::source::stable_id),
        asset,
        folder,
        keys,
        paths,
    })
}
impl Shell {
    pub(super) fn start_asset_operation(
        &mut self,
        editor: &mut Editor,
        plan: plan::Plan,
    ) -> ara_ui::Result<()> {
        if self.asset_ops.busy() {
            return Err("Wait for the current asset preparation to complete".into());
        }
        let selection = Selection::read(self, editor);
        let result = editor.action(
            "cmd.apply",
            json!({"doc":"ara.project.json","commands":plan.calls,"why":plan.title}),
        )?;
        let id = backend::accepted(&result)
            .ok_or("The typed asset command did not return its preparation job")?
            .to_owned();
        self.asset_ops.job = Some(Job {
            id,
            status: json!({"status":"running","progress":0,"message":"Preparing asset transaction…"}),
            polled: Instant::now(),
            began: Instant::now(),
            plan,
            selection,
            following: false,
            cancel_requested: false,
        });
        self.asset_ops.revision += 1;
        self.menu = None;
        Ok(())
    }
    pub(crate) fn cancel_asset_operation(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let Some(job) = &self.asset_ops.job else {
            return Ok(());
        };
        let status = editor.action("jobs.status", json!({"job":job.id}))?;
        if !backend::can_cancel(&status) {
            if let Some(job) = &mut self.asset_ops.job {
                if backend::running(&job.status) && status["status"] == "done" {
                    job.following = true;
                    job.began = Instant::now();
                    self.browser.refresh();
                }
                job.status = status;
            }
            self.status_note =
                Some("The asset transaction is committing or has already finished".into());
            self.asset_ops.revision += 1;
            return Ok(());
        }
        let response = editor.action("jobs.cancel", json!({"job":job.id}))?;
        if let Some(job) = &mut self.asset_ops.job {
            job.cancel_requested = response["cancelled"] == true;
            job.status = status;
            job.polled = Instant::now() - Duration::from_millis(100);
        }
        self.asset_ops.revision += 1;
        Ok(())
    }
    pub(crate) fn dismiss_asset_operation(&mut self) {
        if !self.asset_ops.busy() {
            self.asset_ops.job = None;
            self.asset_ops.revision += 1;
        }
    }
    pub(crate) fn poll_asset_operations(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = self.observe_asset_history(editor);
        changed |= self.follow_asset_history(editor)?;
        if let Some(problem) = self.browser.diagnostic.take() {
            let _ = editor.action("diag.report", json!({"diagnostics":[problem]}));
        }
        if self
            .asset_ops
            .alert
            .as_mut()
            .is_some_and(|alert| alert.try_take().is_some())
        {
            self.asset_ops.alert = None;
            changed = true;
        }
        let answer = self
            .asset_ops
            .confirmation
            .as_mut()
            .and_then(|pending| pending.response.try_take());
        if let Some(accepted) = answer {
            let pending = self
                .asset_ops
                .confirmation
                .take()
                .ok_or("Asset confirmation was removed")?;
            if accepted && let Err(error) = self.start_asset_operation(editor, pending.plan) {
                self.alert_error(editor, &*error);
            }
            changed = true;
        }
        let answer = self
            .asset_ops
            .prompt
            .as_mut()
            .and_then(|prompt| match prompt {
                Prompt::Folder { response, .. } | Prompt::Move { response, .. } => {
                    response.try_take()
                }
            });
        if let Some(answer) = answer {
            let prompt = self
                .asset_ops
                .prompt
                .take()
                .ok_or("Asset prompt was removed")?;
            if let Some(value) = answer {
                let plan = match prompt {
                    Prompt::Folder { parent, .. } => plan::folder(&parent, value.trim()),
                    Prompt::Move { sources, .. } => {
                        let folder = value.trim();
                        let exists = folder.is_empty()
                            || self.browser.items.iter().any(|asset| {
                                asset.kind == "folder"
                                    && asset.authored_path.as_deref() == Some(folder)
                            });
                        if exists {
                            plan::move_to(sources, folder)
                        } else {
                            Err("Choose an existing authored folder shown in Assets".into())
                        }
                    }
                };
                if let Err(error) = plan.and_then(|plan| self.start_asset_operation(editor, plan)) {
                    self.alert_error(editor, &*error);
                }
            }
            changed = true;
        }
        let Some(mut job) = self.asset_ops.job.take() else {
            self.asset_ops.revision += u64::from(changed);
            return Ok(changed);
        };
        if backend::running(&job.status) && job.polled.elapsed() >= Duration::from_millis(100) {
            let status = match editor.action("jobs.status", json!({"job":job.id})) {
                Ok(status) => status,
                Err(error) => {
                    self.asset_ops.job = Some(job);
                    return Err(error.into());
                }
            };
            job.polled = Instant::now();
            if status != job.status {
                let was_running = backend::running(&job.status);
                job.status = status;
                changed = true;
                if job.status["status"] == "done" && was_running {
                    self.browser.refresh();
                    job.following = true;
                    job.began = Instant::now();
                } else if !backend::running(&job.status) {
                    job.following = false;
                    let message = backend::message(&job.status, &job.plan.labels);
                    self.status_note = Some(message.clone());
                    if job.status["status"] == "failed" {
                        if let Some(diagnostics) = job.status["diagnostics"].as_array() {
                            let _ =
                                editor.action("diag.report", json!({"diagnostics":diagnostics}));
                        }
                        self.asset_ops.alert=Some(self.confirmations.alert(DialogOptions::new("Asset transaction was not applied",format!("{message}\n\nResolve the listed blocker before trying again. Your current selection remains available."))));
                    }
                }
            }
        }
        if job.status["status"] == "done"
            && let Some(txn) = backend::transaction(&job.status)
        {
            if self.asset_ops.history.applied(txn).is_none() {
                let id = ara_ops::TxnId(txn.into());
                if let Some(revision) = editor
                    .host
                    .store()
                    .txn(&id)
                    .map(|transaction| transaction.result)
                    && let Some(after) = planned_selection(
                        &job.selection,
                        &job.plan,
                        &self.browser.items,
                        |path, items| projected(editor.host.store().workspace(), path, items),
                    )
                {
                    self.asset_ops
                        .history
                        .remember(id, revision, job.selection.clone(), after);
                    changed |= self.observe_asset_history(editor);
                }
            }
            if self.asset_ops.history.applied(txn) == Some(false)
                || self.asset_ops.history.pending()
            {
                job.following = false;
            }
        }
        if job.following {
            if !job.selection.unchanged(self, editor) {
                job.following = false;
                changed = true;
            } else if let Some(after) =
                planned_selection(&job.selection, &job.plan, &self.browser.items, resolve)
            {
                if let Err(error) = editor.action(
                    "editor.workspace",
                    json!({"folder":after.folder,"asset":after.asset.as_deref().unwrap_or("")}),
                ) {
                    self.asset_ops.job = Some(job);
                    return Err(error.into());
                }
                self.asset_selection.set_selected(after.keys);
                if let Some(active) = after.active {
                    self.asset_selection.focus(active);
                }
                self.asset_selection_path = after.primary;
                self.asset_grid_dirty = true;
                job.following = false;
                changed = true;
                self.status_note = Some(backend::message(&job.status, &job.plan.labels));
            } else if job.began.elapsed() > Duration::from_secs(30) {
                // File success is never reinterpreted as failure or optimistically
                // projected. Let the next semantic scan reconcile stale keys.
                job.following = false;
                changed = true;
                self.status_note=Some("Asset transaction completed; waiting for its semantic browser entries to refresh".into());
            }
        }
        self.asset_ops.job = Some(job);
        self.asset_ops.revision += u64::from(changed);
        Ok(changed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_selection_rewrites_only_its_subtree_and_delete_preserves_other_assets() {
        let folder = assets::Asset::folder("game/Art".into(), Some("Art".into()));
        let changes = vec![(folder.clone(), Some("Archive/Art".into()))];
        assert!(
            matches!(relocated("game/Art/a.scene",&changes),Some(Desired::Authored(path)) if path=="Archive/Art/a.scene")
        );
        assert!(
            matches!(relocated("game/Artist/a.scene",&changes),Some(Desired::Storage(path)) if path=="game/Artist/a.scene")
        );
        assert!(relocated("game/Art/a.scene", &[(folder, None)]).is_none());
    }
    #[test]
    fn future_storage_keys_use_the_deepest_authored_parent_not_a_project_collision() {
        let items = vec![
            assets::Asset::folder("settings/Archive".into(), None),
            assets::Asset::folder("game/Archive".into(), Some("Archive".into())),
            assets::Asset::folder("game/Archive/Nested".into(), Some("Archive/Nested".into())),
        ];
        let mut workspace = ara_doc::Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        assert_eq!(
            projected(
                &workspace,
                Desired::Authored("Archive/Nested/A.prefab".into()),
                &items
            )
            .as_deref(),
            Some("game/Archive/Nested/A.prefab")
        );
        assert!(resolve(Desired::Authored("Archive/Nested/A.prefab".into()), &items).is_none());
        assert_eq!(
            projected(&workspace, Desired::Authored("New.prefab".into()), &items),
            Some(workspace.storage_path("New.prefab"))
        );
        let legacy = ara_doc::Workspace::default();
        assert_eq!(
            projected(&legacy, Desired::Authored("New.prefab".into()), &[]),
            Some("New.prefab".into())
        );
    }
}
