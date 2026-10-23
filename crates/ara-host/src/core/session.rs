//! `session.*`, `history.*` and `review.*`.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, codes};
use ara_doc::Op;
use ara_ops::{Author, Transaction, TxnId, TxnMeta};
use ara_proto::Scope;
use serde_json::{Value, json};

use super::pick::find_doc;
use crate::host::{State, session_not_found};
use crate::journal::Entry;
use crate::params::*;
use crate::review::{entity_key, touched};
use crate::session::{ApprovalRequest, Checkpoint, Session, default_author};
use crate::text::short;

type R = Result<Value, Vec<Diagnostic>>;

fn session<'a>(st: &'a State, sid: &str) -> Result<&'a Session, Vec<Diagnostic>> {
    st.sessions
        .live(sid)
        .ok_or_else(|| vec![session_not_found(sid)])
}

pub(super) fn begin(st: &mut State, p: SessionBeginParams) -> R {
    let author = match &p.author {
        Some(a) => a
            .parse::<Author>()
            .map_err(|d| vec![d.path("params.author")])?,
        None => default_author(&p.client),
    };
    let requested = p
        .scopes
        .as_ref()
        .map(|list| {
            list.iter()
                .map(|s| s.parse::<Scope>())
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|d| vec![d.path("params.scopes")])?;
    let decision = st.approval.decide(&ApprovalRequest {
        client: &p.client,
        author: &author,
        requested: requested.as_deref(),
        label: p.label.as_deref(),
    });
    let denied: Vec<Scope> = requested
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !decision.granted.contains(s))
        .collect();
    let session = Session {
        id: st.token("ses"),
        client: p.client,
        author,
        label: p.label,
        granted: decision.granted,
        denied,
        approved_by: decision.by,
        started: st.store.revision(),
        checkpoints: Vec::new(),
        ended: false,
    };
    st.journal(&Entry::Session {
        session: session.clone(),
    });
    let names = |s: &[Scope]| s.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", ");
    st.log(Diagnostic::info(
        codes::SESSION_APPROVED,
        format!(
            "session `{}` for `{}` ({}) approved by {}: [{}]",
            session.id,
            session.client,
            session.author,
            session.approved_by,
            names(&session.granted)
        ),
    ));
    if !session.denied.is_empty() {
        st.log(Diagnostic::warning(
            codes::SCOPE_DENIED,
            format!(
                "session `{}`: scopes [{}] were not granted",
                session.id,
                names(&session.denied)
            ),
        ));
    }
    let ws = st.store.workspace();
    let documents: Vec<Value> = ws
        .docs()
        .map(|(path, d)| json!({ "path": path, "kind": d.kind, "id": d.id }))
        .collect();
    let out = json!({
        "session": session.id,
        "author": session.author,
        "granted": session.granted,
        "denied": session.denied,
        "approvedBy": session.approved_by,
        "revision": session.started,
        "documents": documents,
        "mainScene": st.main,
        "hint": "Pass `session` to every call. Find commands and components with schema.search, entities with query, docs with docs.search. Edit with cmd.apply: atomic batches with a `why`; `doc` defaults to the document holding the entities you address, else the main scene. Entities: IDs (`e_8b21`), `$symbols` from earlier calls, or `name:Crate` / `name:Floor/Crate`. Y-up, meters, degrees.",
    });
    st.sessions.insert(session);
    Ok(out)
}

/// Transactions made by session `sid`, oldest first.
fn session_txns<'a>(st: &'a State, sid: &str) -> Vec<&'a Transaction> {
    st.store
        .history()
        .iter()
        .filter(|t| st.txn_session.get(&t.id).is_some_and(|s| s == sid))
        .collect()
}

pub(super) fn status(st: &mut State, sid: &str) -> R {
    let s = session(st, sid)?;
    let txns = session_txns(st, sid);
    let unreviewed = txns
        .iter()
        .filter(|t| st.review.status(&t.id) == Some(ReviewStatus::Applied))
        .count();
    Ok(json!({
        "session": s,
        "revision": st.store.revision(),
        "transactions": txns.len(),
        "unreviewed": unreviewed,
        "files": st.file_status(),
    }))
}

#[derive(Default)]
struct Change {
    created: bool,
    deleted: bool,
    fields: BTreeMap<String, (Option<Value>, Option<Value>)>,
}

fn note(
    fields: &mut BTreeMap<String, (Option<Value>, Option<Value>)>,
    path: String,
    was: Option<Value>,
    now: Option<Value>,
) {
    fields.entry(path).or_insert((was, None)).1 = now;
}

pub(super) fn diff(st: &mut State, sid: &str, p: SessionDiffParams) -> R {
    let s = session(st, sid)?;
    let since = match &p.checkpoint {
        Some(cp) => s.checkpoint(cp).map(|c| c.revision).ok_or_else(|| {
            vec![Diagnostic::error(
                codes::CHECKPOINT_NOT_FOUND,
                format!("session `{sid}` has no checkpoint `{cp}`"),
            )]
        })?,
        None => s.started,
    };
    let mut changes: BTreeMap<(String, String), Change> = BTreeMap::new();
    for t in session_txns(st, sid) {
        if t.result <= since || t.reverts.is_some() || st.store.reverted_by(&t.id).is_some() {
            continue;
        }
        for op in &t.ops {
            let key = (op.doc().to_string(), op.id().to_string());
            let c = changes.entry(key.clone()).or_default();
            match op {
                Op::Create { .. } => {
                    c.created = !c.deleted;
                    c.deleted = false;
                }
                Op::Delete { .. } if c.created => {
                    changes.remove(&key);
                }
                Op::Delete { .. } => c.deleted = true,
                Op::Set {
                    path, value, prev, ..
                } => note(&mut c.fields, path.to_string(), prev.clone(), value.clone()),
                Op::Move {
                    parent,
                    order,
                    prev_parent,
                    prev_order,
                    ..
                } => {
                    note(
                        &mut c.fields,
                        "parent".into(),
                        prev_parent.clone().map(Value::from),
                        parent.clone().map(Value::from),
                    );
                    let ord = |o: &Option<ara_doc::OrderKey>| {
                        o.as_ref().map(|k| Value::from(k.to_string()))
                    };
                    note(&mut c.fields, "order".into(), ord(prev_order), ord(order));
                }
            }
        }
    }
    let mut items = Vec::new();
    let mut text = String::new();
    for ((doc, record), c) in &changes {
        let kind = if c.created {
            "created"
        } else if c.deleted {
            "deleted"
        } else {
            "modified"
        };
        let fields: BTreeMap<&String, Value> = c
            .fields
            .iter()
            .filter(|(_, (was, now))| was != now)
            .map(|(k, (was, now))| (k, json!({ "was": was, "now": now })))
            .collect();
        if kind == "modified" && fields.is_empty() {
            continue;
        }
        let mark = match kind {
            "created" => '+',
            "deleted" => '-',
            _ => '~',
        };
        text.push_str(&format!("{mark} {doc}/{record}"));
        if kind == "modified" {
            let parts: Vec<String> = c
                .fields
                .iter()
                .filter(|(_, (w, n))| w != n)
                .map(|(k, (w, n))| {
                    let show = |v: &Option<Value>| v.as_ref().map_or("-".to_owned(), short);
                    format!("{k}: {} → {}", show(w), show(n))
                })
                .collect();
            text.push_str(&format!(" {}", parts.join("; ")));
        } else {
            text.push_str(&format!(" ({kind})"));
        }
        text.push('\n');
        items.push(json!({ "doc": doc, "entity": record, "change": kind, "fields": fields }));
    }
    if text.is_empty() {
        text = format!("no changes since revision {since}\n");
    }
    Ok(json!({ "since": since, "revision": st.store.revision(), "changes": items, "text": text }))
}

/// Records a checkpoint at `revision` (default: now) on session `sid`.
pub(crate) fn checkpoint_at(
    st: &mut State,
    sid: &str,
    revision: u64,
    label: Option<String>,
    auto: bool,
) -> Checkpoint {
    let cp = Checkpoint {
        id: st.token("cp"),
        revision,
        label,
        auto,
    };
    st.journal(&Entry::Checkpoint {
        session: sid.to_owned(),
        checkpoint: cp.clone(),
    });
    if let Some(s) = st.sessions.get_mut(sid) {
        s.checkpoints.push(cp.clone());
    }
    cp
}

pub(super) fn checkpoint(st: &mut State, sid: &str, label: Option<String>, auto: bool) -> Value {
    let rev = st.store.revision();
    json!(checkpoint_at(st, sid, rev, label, auto))
}

pub(super) fn rollback(st: &mut State, sid: &str, p: RollbackParams) -> R {
    let s = session(st, sid)?;
    let cp = s.checkpoint(&p.to).cloned().ok_or_else(|| {
        vec![Diagnostic::error(
            codes::CHECKPOINT_NOT_FOUND,
            format!("session `{sid}` has no checkpoint `{}`", p.to),
        )]
    })?;
    let author = s.author.clone();
    st.cancel_held_from(sid, cp.revision);
    let targets: Vec<TxnId> = session_txns(st, sid)
        .into_iter()
        .rev()
        .filter(|t| {
            t.result > cp.revision && t.reverts.is_none() && st.store.reverted_by(&t.id).is_none()
        })
        .map(|t| t.id.clone())
        .collect();
    let mut undone: Vec<TxnId> = Vec::new();
    for id in &targets {
        match st.store.undo(id, author.clone()) {
            Ok(t) => undone.push(t.id.clone()),
            Err(diags) => {
                // All or nothing: undo the undos (newest first) before failing.
                for u in undone.iter().rev() {
                    let _ = st.store.undo(u, author.clone());
                }
                return Err(diags);
            }
        }
    }
    Ok(json!({ "to": cp.id, "reverted": targets, "revision": st.store.revision() }))
}

pub(super) fn end(st: &mut State, sid: &str) -> R {
    session(st, sid)?;
    if let Some(s) = st.sessions.get_mut(sid) {
        s.ended = true;
    }
    st.journal(&Entry::Ended {
        session: sid.to_owned(),
    });
    Ok(json!({ "session": sid, "ended": true }))
}

fn history_line(st: &State, t: &Transaction) -> String {
    let status = st.review.status(&t.id).map_or(String::new(), |s| {
        format!(
            " {}",
            serde_json::to_value(s)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        )
    });
    let what = t.why.as_deref().or(t.label.as_deref()).unwrap_or("");
    format!(
        "{} r{} {}{status} · {what} · {} entities\n",
        t.id,
        t.result,
        t.author,
        touched(t).len()
    )
}

pub(super) fn history(st: &mut State, p: HistoryListParams) -> R {
    let author = p
        .author
        .as_deref()
        .map(str::parse::<Author>)
        .transpose()
        .map_err(|d| vec![d.path("params.author")])?;
    let doc = p.doc.as_deref().map(|d| find_doc(st, d)).transpose()?;
    let limit = p.limit.unwrap_or(20).clamp(1, 500) as usize;
    let picked: Vec<&Transaction> = st
        .store
        .history()
        .iter()
        .rev()
        .filter(|t| author.as_ref().is_none_or(|a| t.author == *a))
        .filter(|t| p.since.is_none_or(|s| t.result > s))
        .filter(|t| {
            doc.as_ref()
                .is_none_or(|d| t.ops.iter().any(|op| op.doc() == d))
        })
        .take(limit)
        .collect();
    let text: String = picked.iter().map(|t| history_line(st, t)).collect();
    let items: Vec<Value> = picked.iter().map(|t| st.txn_summary(t)).collect();
    Ok(json!({ "revision": st.store.revision(), "items": items, "text": text }))
}

fn txn_json(t: &Transaction) -> Value {
    json!({ "txn": t.id, "reverts": t.reverts, "revision": t.result, "label": t.label })
}

pub(super) fn undo(st: &mut State, sid: &str, p: UndoParams) -> R {
    let author = session(st, sid)?.author.clone();
    let t = match p.txn {
        Some(id) => st.store.undo(&TxnId(id), author)?,
        None => st.store.undo_last(&author)?,
    };
    Ok(txn_json(t))
}

pub(super) fn redo(st: &mut State, sid: &str) -> R {
    let author = session(st, sid)?.author.clone();
    let t = st.store.redo_last(&author)?;
    Ok(txn_json(t))
}

pub(super) fn review_list(st: &mut State, p: ReviewListParams) -> R {
    let want = p.status.unwrap_or(ReviewStatus::Applied);
    let limit = p.limit.unwrap_or(50).clamp(1, 500) as usize;
    let picked: Vec<&Transaction> = st
        .store
        .history()
        .iter()
        .rev()
        .filter(|t| st.review.status(&t.id) == Some(want))
        .take(limit)
        .collect();
    let text: String = picked.iter().map(|t| history_line(st, t)).collect();
    let items: Vec<Value> = picked
        .iter()
        .map(|t| {
            let mut v = st.txn_summary(t);
            v["review"] = json!(st.review.entities(&t.id));
            v
        })
        .collect();
    Ok(json!({ "status": want, "items": items, "text": text }))
}

/// The (transaction, entity keys) a review call targets, among entities whose status
/// is in `from`.
fn targets(
    st: &State,
    p: &ReviewParams,
    from: &[ReviewStatus],
) -> Result<Vec<(TxnId, Vec<String>)>, Vec<Diagnostic>> {
    let pick = |id: &TxnId| -> Vec<String> {
        st.review
            .entities(id)
            .map(|e| {
                e.iter()
                    .filter(|(key, s)| {
                        from.contains(s)
                            && p.entity.as_ref().is_none_or(|want| {
                                *key == want || key.ends_with(&format!("/{want}"))
                            })
                    })
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let ids: Vec<TxnId> = match (&p.txn, &p.entity, p.all.unwrap_or(false)) {
        (Some(t), _, _) => {
            let id = TxnId(t.clone());
            if !st.review.is_tracked(&id) {
                return Err(vec![Diagnostic::error(
                    codes::NOT_REVIEWABLE,
                    format!("`{t}` is not an agent transaction under review"),
                )]);
            }
            if let Some((group, _)) = st
                .store
                .txn(&id)
                .and_then(|t| crate::gesture::turn(t.label.as_deref()))
            {
                st.store
                    .history()
                    .iter()
                    .rev()
                    .filter(|t| {
                        crate::gesture::turn(t.label.as_deref()).is_some_and(|(id, _)| id == group)
                    })
                    .map(|t| t.id.clone())
                    .collect()
            } else {
                vec![id]
            }
        }
        (None, Some(_), _) | (None, None, true) => st
            .store
            .history()
            .iter()
            .rev()
            .filter(|t| st.review.is_tracked(&t.id))
            .map(|t| t.id.clone())
            .collect(),
        (None, None, false) => {
            return Err(vec![Diagnostic::error(
                codes::INVALID_PARAMS,
                "give `txn`, `entity` or `all: true`",
            )]);
        }
    };
    let out: Vec<(TxnId, Vec<String>)> = ids
        .into_iter()
        .map(|id| {
            let keys = pick(&id);
            (id, keys)
        })
        .filter(|(_, keys)| !keys.is_empty())
        .collect();
    if out.is_empty() {
        return Err(vec![Diagnostic::error(
            codes::NOT_REVIEWABLE,
            "nothing matching is left to review",
        )]);
    }
    Ok(out)
}

fn record_review(st: &mut State, txn: &TxnId, entity: Option<&str>, status: ReviewStatus) {
    st.review.set(txn, entity, status);
    st.journal(&Entry::Review {
        txn: txn.0.clone(),
        entity: entity.map(str::to_owned),
        status,
    });
}

pub(super) fn accept(st: &mut State, sid: &str, p: ReviewParams) -> R {
    if session(st, sid)?.author != Author::Human {
        return Err(vec![Diagnostic::error(
            codes::NOT_REVIEWABLE,
            "only human sessions accept agent changes; agents can revert (`review.revert`) or undo their own",
        )]);
    }
    let picked = targets(st, &p, &[ReviewStatus::Applied])?;
    let mut done = Vec::new();
    for (txn, keys) in picked {
        for key in &keys {
            record_review(st, &txn, Some(key), ReviewStatus::Accepted);
        }
        let summary = st.store.txn(&txn).map(|t| st.txn_summary(t));
        if let Some(s) = summary {
            st.notify(Topic::History, s);
        }
        done.push(json!({ "txn": txn, "entities": keys, "status": st.review.status(&txn) }));
    }
    Ok(json!({ "accepted": done }))
}

pub(super) fn revert(st: &mut State, sid: &str, p: ReviewParams) -> R {
    let author = session(st, sid)?.author.clone();
    let picked = targets(st, &p, &[ReviewStatus::Applied, ReviewStatus::Accepted])?;
    if let Some(group) = p
        .txn
        .as_ref()
        .and_then(|id| st.store.txn(&TxnId(id.clone())))
        .and_then(|t| crate::gesture::turn(t.label.as_deref()))
        .map(|(id, _)| id.to_owned())
    {
        st.cancel_held_turn(&group);
    }
    let mut done = Vec::new();
    for (txn, keys) in picked {
        let Some(target) = st.store.txn(&txn).cloned() else {
            continue;
        };
        let whole = keys.len() == touched(&target).len() && st.store.reverted_by(&txn).is_none();
        let by = if whole {
            st.store.undo(&txn, author.clone())?.id.clone()
        } else {
            let ops: Vec<&Op> = target
                .ops
                .iter()
                .filter(|op| keys.contains(&entity_key(op.doc().as_str(), op.id().as_str())))
                .collect();
            let meta = TxnMeta {
                author: author.clone(),
                why: Some(format!("revert {} from `{txn}`", keys.join(", "))),
                label: Some(format!("revert {txn}")),
                base: Some(target.result),
                idempotency_key: None,
            };
            let mut staged = st.store.begin(meta);
            for op in ops.iter().rev() {
                staged.apply(op.inverse().to_edit()).map_err(|d| vec![d])?;
            }
            let id = staged.commit()?.txn().map(|t| t.id.clone());
            // Compensating transactions need no review of their own.
            st.untracked = true;
            st.drain();
            st.untracked = false;
            for key in &keys {
                record_review(st, &txn, Some(key), ReviewStatus::Reverted);
            }
            match id {
                Some(id) => id,
                None => continue,
            }
        };
        done.push(json!({ "txn": txn, "entities": keys, "by": by }));
    }
    st.drain();
    Ok(json!({ "reverted": done, "revision": st.store.revision() }))
}
