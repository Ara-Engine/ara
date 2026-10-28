//! A migration is a history boundary, never a second live authoring root.
use crate::host::State;
use ara_diag::{Diagnostic, codes};
use ara_doc::Op;
use ara_ops::Transaction;
use serde_json::Value;

fn boundary(txn: &Transaction) -> bool {
    txn.ops.iter().any(|op| {
        matches!(op,Op::Set{doc,id,path,value:Some(value),prev,..}
        if doc.as_str()=="pj_project" && id.as_str()=="p_settings" && path.to_string()=="settings"
        && ara_doc::detect_project_layout(value).is_current()
        && prev.as_ref().is_some_and(|before|!ara_doc::detect_project_layout(before).is_current()))
    }) && txn.ops.iter().any(|op| matches!(op, Op::File { .. }))
}

pub(crate) fn check(st: &State, method: &str, params: &Value) -> Result<(), Vec<Diagnostic>> {
    let marker = st
        .store
        .workspace()
        .doc("pj_project")
        .and_then(|doc| doc.record("p_settings"))
        .and_then(|record| record.get("settings"));
    let current = marker.is_some_and(|value| ara_doc::detect_project_layout(value).is_current());
    let author = params["session"]
        .as_str()
        .and_then(|sid| st.sessions.live(sid))
        .map(|s| &s.author);
    let migration_call = method == "cmd.apply"
        && params["commands"]
            .as_array()
            .is_some_and(|calls| calls.len() == 1 && calls[0]["cmd"] == "project.migrate");
    let migration_redo = method == "history.redo"
        && author
            .and_then(|a| st.store.next_redo(a))
            .and_then(|undo| undo.reverts.as_ref())
            .and_then(|id| st.store.txn(id))
            .is_some_and(boundary);
    if !current
        && !migration_call
        && !migration_redo
        && !matches!(
            method,
            "rpc.discover"
                | "session.begin"
                | "session.status"
                | "session.end"
                | "history.list"
                | "jobs.list"
                | "jobs.status"
                | "jobs.cancel"
                | "logs"
                | "diag.query"
                | "diag.subscribe"
                | "diag.clear"
                | "daemon.shutdown"
        )
    {
        return Err(vec![Diagnostic::error(
            codes::PROJECT_MIGRATION_REQUIRED,
            "the project has an old layout, possibly restored by Undo; close it and migrate before authoring, or Redo this migration in the same authority",
        )]);
    }
    let Some(epoch) = st
        .store
        .history()
        .iter()
        .rev()
        .find(|txn| boundary(txn) && st.store.undoable(&txn.id))
    else {
        return Ok(());
    };
    let target = match method {
        "history.undo" => params["txn"]
            .as_str()
            .and_then(|id| st.store.txn(&ara_ops::TxnId(id.into())))
            .or_else(|| author.and_then(|a| st.store.next_undo(a))),
        "history.redo" => author.and_then(|a| st.store.next_redo(a)),
        "review.revert" => params["txn"]
            .as_str()
            .and_then(|id| st.store.txn(&ara_ops::TxnId(id.into()))),
        _ => None,
    };
    if target.is_some_and(|txn| txn.result < epoch.result) {
        return Err(vec![Diagnostic::error(
            codes::STALE_BASE,
            "this transaction predates the project-layout migration; Undo the migration first to restore its original paths",
        )]);
    }
    if method == "review.revert" && params["txn"].is_null() {
        return Err(vec![Diagnostic::error(
            codes::STALE_BASE,
            "after a project migration, select a review transaction from the current layout or Undo the migration first",
        )]);
    }
    if method == "session.rollback"
        && params["to"]
            .as_str()
            .and_then(|to| {
                params["session"]
                    .as_str()
                    .and_then(|sid| st.sessions.live(sid))
                    .and_then(|s| s.checkpoint(to))
            })
            .is_some_and(|cp| cp.revision < epoch.result)
    {
        return Err(vec![Diagnostic::error(
            codes::STALE_BASE,
            "rollback crosses a project migration; Undo the migration first",
        )]);
    }
    Ok(())
}
