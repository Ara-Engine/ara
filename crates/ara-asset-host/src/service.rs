//! Cancellable asset preparation around the host's normal typed transaction.
mod authorization;
mod bindings;
mod import_bindings;
mod import_context;
mod import_entries;
mod import_graph;
mod import_hashes;
mod import_plan;
mod import_projection;
mod importing;
mod inventory;
mod links;
mod metadata;
mod prepare;
mod profiles;
mod sidecars;
mod ui_paths;

use ara_asset::{Cancellation, ProcessWorker};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::PreparedInventory;
use ara_host::{Cx, Method, Service, params::ApplyParams};
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};

/// Disk admission supplied by the embedding application's platform adapter.
/// It must check the actual staging volume and report insufficient space.
pub type SpaceCheck = dyn Fn(&Path, u64) -> ara_asset::Result<()> + Send + Sync;

enum Event {
    Progress(f32, String),
    Ready(Result<PreparedInventory, Vec<Diagnostic>>),
}

struct Pending {
    job: String,
    session: String,
    revision: u64,
    params: Option<ApplyParams>,
    cancellation: Cancellation,
    events: Receiver<Event>,
    commit: Option<(ara_host::FileCommitReceipt, Value)>,
    profile: bool,
    idempotency_key: Option<String>,
}

/// Prepares asset batches away from the authority thread, then commits under the
/// initiating session. Register one service on both CLI and editor hosts.
pub struct AssetService {
    worker: ProcessWorker,
    check_space: Arc<SpaceCheck>,
    pending: Vec<Pending>,
}

impl AssetService {
    /// Construct without I/O. The caller supplies its isolated importer executable
    /// and real disk-space check; there is no success-by-default disk policy.
    pub fn new(worker: ProcessWorker, check_space: Arc<SpaceCheck>) -> Self {
        Self {
            worker,
            check_space,
            pending: Vec::new(),
        }
    }

    fn start(&mut self, cx: &mut Cx<'_>, params: &ApplyParams) -> Result<Value, Vec<Diagnostic>> {
        authorization::authorize(cx, params)?;
        let mut params: ApplyParams = serde_json::to_value(params)
            .and_then(serde_json::from_value)
            .map_err(|error| vec![Diagnostic::error(codes::INVALID_PARAMS, error.to_string())])?;
        if params.dry_run.unwrap_or(false) {
            for call in &mut params.commands {
                if call.cmd == "asset.import" {
                    call.args.get_or_insert_with(|| json!({}))["dryRun"] = json!(true);
                }
            }
        }
        let frozen = prepare::Frozen {
            root: cx.root().to_owned(),
            workspace: cx.store().workspace().clone(),
            schema: cx.store().schema().clone(),
            batch: cx.command_batch(&params)?.1,
            worker: self.worker.clone(),
            check_space: Arc::clone(&self.check_space),
        };
        let cancellation = Cancellation::default();
        let worker_cancel = cancellation.clone();
        let worker_params = serde_json::to_value(&params)
            .and_then(serde_json::from_value)
            .map_err(|error| vec![Diagnostic::error(codes::INVALID_PARAMS, error.to_string())])?;
        let session = cx
            .session()
            .map(|session| session.id.clone())
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::SESSION_NOT_FOUND,
                    "asset preparation needs a live session",
                )]
            })?;
        let (send, events) = mpsc::sync_channel(64);
        std::thread::Builder::new()
            .name("ara-asset-prepare".into())
            .spawn(move || {
                let result = prepare::run(
                    frozen,
                    worker_params,
                    &worker_cancel,
                    &mut |fraction, message| {
                        let _ = send.send(Event::Progress(fraction, message.to_owned()));
                    },
                );
                let _ = send.send(Event::Ready(result));
            })
            .map_err(|error| vec![Diagnostic::error(codes::IO, error.to_string())])?;
        let profile = params
            .commands
            .iter()
            .all(|call| call.cmd == "build_profile.create");
        let job = cx.start_job(
            if params
                .commands
                .iter()
                .any(|call| call.cmd == "asset.import")
            {
                "asset.import"
            } else if profile {
                "build_profile.create"
            } else {
                "asset.prepare"
            },
        );
        let idempotency_key = params.idempotency_key.clone();
        self.pending.push(Pending {
            job: job.clone(),
            session,
            revision: cx.store().revision(),
            params: Some(params),
            cancellation,
            events,
            commit: None,
            profile,
            idempotency_key,
        });
        Ok(json!({"ok":true,"job":job,"preparing":true}))
    }
}

impl Service for AssetService {
    fn name(&self) -> &'static str {
        "assets"
    }

    fn methods(&self, _: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }

    fn call(&mut self, _: &mut Cx<'_>, method: &str, _: Value) -> Result<Value, Vec<Diagnostic>> {
        Err(vec![Diagnostic::error(
            codes::UNKNOWN_METHOD,
            format!("asset service has no method `{method}`; use cmd.apply"),
        )])
    }

    fn prepare_apply(
        &mut self,
        cx: &mut Cx<'_>,
        params: &ApplyParams,
    ) -> Option<Result<Value, Vec<Diagnostic>>> {
        if cx.store().workspace().prepared_files().is_some()
            || !params.commands.iter().any(|call| {
                call.cmd.starts_with("asset.")
                    || matches!(call.cmd.as_str(), "folder.create" | "build_profile.create")
            })
        {
            return None;
        }
        if let Some(key) = params.idempotency_key.as_deref()
            && let Some(pending) = self.pending.iter().find(|pending| {
                pending.idempotency_key.as_deref() == Some(key) && !cx.job_cancelled(&pending.job)
            })
        {
            return Some(authorization::authorize(cx,params).map(|()|json!({"ok":true,"job":pending.job,"preparing":pending.commit.is_none(),"replayed":true})));
        }
        if params
            .idempotency_key
            .as_deref()
            .is_some_and(|key| cx.store().find_by_key(key).is_some())
        {
            return None;
        }
        Some(self.start(cx, params))
    }

    fn tick(&mut self, cx: &mut Cx<'_>) {
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.pending) {
            if let Some((receipt, _)) = pending.commit.as_ref() {
                if let Some(saved) = cx.poll_file_commit(*receipt) {
                    let (_, result) = pending
                        .commit
                        .take()
                        .expect("a committing asset job retains its transaction result");
                    cx.finish_job(&pending.job, saved.map(|()| result));
                } else {
                    waiting.push(pending);
                }
                continue;
            }
            let cancelled = cx.job_cancelled(&pending.job);
            if cancelled {
                pending.cancellation.cancel();
            }
            let mut ready = None;
            for _ in 0..64 {
                match pending.events.try_recv() {
                    Ok(Event::Progress(fraction, message)) => {
                        if !cancelled {
                            cx.job_progress(&pending.job, fraction.clamp(0.0, 0.98), &message);
                        }
                    }
                    Ok(Event::Ready(result)) => {
                        ready = Some(result);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        ready = Some(Err(vec![Diagnostic::error(
                            codes::IO,
                            "asset preparation worker disconnected",
                        )]));
                        break;
                    }
                }
            }
            let Some(ready) = ready else {
                waiting.push(pending);
                continue;
            };
            if cancelled {
                if let Err(diagnostics) = ready {
                    for diagnostic in diagnostics.into_iter().filter(Diagnostic::is_error) {
                        cx.log(diagnostic);
                    }
                }
                continue;
            }
            let result = ready.and_then(|inventory| {
                cx.job_progress(
                    &pending.job,
                    0.99,
                    if pending.profile {
                        "Creating build profile"
                    } else {
                        "Committing prepared assets"
                    },
                );
                let params = pending
                    .params
                    .take()
                    .expect("a preparing asset job retains its original batch");
                cx.resume_session(&pending.session, |cx| {
                    cx.apply_prepared_job(&pending.job, params, inventory, pending.revision)
                })?
            });
            match result {
                Ok((result, receipt)) => {
                    cx.job_progress(
                        &pending.job,
                        0.99,
                        if pending.profile {
                            "Saving build profile"
                        } else {
                            "Saving prepared assets"
                        },
                    );
                    pending.commit = Some((receipt, result));
                    waiting.push(pending);
                }
                Err(diagnostics) => cx.finish_job(&pending.job, Err(diagnostics)),
            }
        }
        self.pending = waiting;
    }
}

impl Drop for AssetService {
    fn drop(&mut self) {
        for pending in &self.pending {
            pending.cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
