//! The dedicated authority exposes no alternate write path: it submits cmd.apply.
use super::{MigrationControl, inventory::Prepared};
use crate::{Cx, FileCommitReceipt, Method, Service, params::ApplyParams};
use ara_diag::{Diagnostic, codes};
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Completion = Arc<Mutex<Option<Result<Value, Vec<Diagnostic>>>>>;
pub(super) struct MigrationService {
    prepared: Option<Prepared>,
    control: MigrationControl,
    completion: Completion,
    pending: Option<(String, FileCommitReceipt, Value)>,
}
impl MigrationService {
    pub(super) fn new(
        prepared: Prepared,
        control: MigrationControl,
        completion: Completion,
    ) -> Self {
        Self {
            prepared: Some(prepared),
            control,
            completion,
            pending: None,
        }
    }
    fn complete(&self, result: Result<Value, Vec<Diagnostic>>) {
        if let Ok(mut completion) = self.completion.lock() {
            *completion = Some(result);
        }
    }
}
impl Service for MigrationService {
    fn name(&self) -> &'static str {
        "project-migration"
    }
    fn methods(&self, _db: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }
    fn call(
        &mut self,
        _cx: &mut Cx<'_>,
        _method: &str,
        _params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        Err(vec![Diagnostic::error(
            codes::UNKNOWN_METHOD,
            "migration uses cmd.apply",
        )])
    }
    fn prepare_apply(
        &mut self,
        cx: &mut Cx<'_>,
        params: &ApplyParams,
    ) -> Option<Result<Value, Vec<Diagnostic>>> {
        if !params
            .commands
            .iter()
            .any(|call| call.cmd == "project.migrate")
        {
            return None;
        }
        Some((|| {
            if params.commands.len() != 1 || params.branch.is_some() {
                return Err(vec![Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "project.migrate must be the only command in its dedicated authority",
                )]);
            }
            let prepared = self.prepared.take().ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::STALE_BASE,
                    "migration authority has already consumed its prepared plan",
                )]
            })?;
            let revision = cx.store().revision();
            if params.dry_run == Some(true) {
                return cx.apply_prepared(
                    serde_json::from_value(
                        serde_json::to_value(params).expect("apply params serialize"),
                    )
                    .expect("same apply params"),
                    prepared.inventory,
                    revision,
                );
            }
            self.control.commit()?;
            let job = cx.start_job("project.migrate");
            let params = serde_json::from_value(
                serde_json::to_value(params).expect("apply params serialize"),
            )
            .expect("same apply params");
            let (value, receipt) =
                cx.apply_prepared_job(&job, params, prepared.inventory, revision)?;
            // This append is ordered after materialization. The receipt's second
            // barrier also covers these baseline entries before success is visible.
            for (doc, hash) in prepared.baselines {
                cx.st.journal(crate::journal::Entry::Flush {
                    doc,
                    rev: cx.store().revision(),
                    hash,
                });
            }
            self.pending = Some((job.clone(), receipt, value));
            Ok(json!({"ok":true,"job":job,"preparing":true}))
        })())
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        let Some((job, receipt, value)) = self.pending.take() else {
            return;
        };
        let Some(result) = cx.poll_file_commit(receipt) else {
            self.pending = Some((job, receipt, value));
            return;
        };
        let result = result.map(|()| {
            let mut value = value;
            value["restartRequired"] = true.into();
            value
        });
        cx.finish_job(&job, result.clone());
        self.complete(result);
    }
}
