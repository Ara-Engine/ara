//! Exact-field gesture protection. Only conflicting ops wait, for at most two seconds.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

use ara_cmd::{Batch, BatchResult};
use ara_diag::{Diagnostic, codes};
use ara_doc::{Edit, Op};
use ara_ops::{Author, TxnMeta};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Cx, Method, Service, host::State, parse};

type Field = (String, String, String);

/// Group split gesture transactions into the original agent turn; preserve its display label.
pub(crate) fn turn(label: Option<&str>) -> Option<(&str, &str)> {
    label?.strip_prefix("gesture-turn:")?.split_once('|')
}

/// Parameters shared by the native inspector, gizmos and remote editors.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GestureParams {
    /// Human editor session owning this gesture.
    pub session: String,
    /// Document path or ID.
    pub doc: String,
    /// Entity ID or prefab address.
    pub entity: String,
    /// Exact reflected field path, such as `Transform.position.x`.
    pub path: String,
    /// Begin/continue the gesture, or end/cancel it.
    pub active: bool,
}

struct HeldResult {
    txn: Option<ara_ops::TxnId>,
    count: usize,
}

struct Held {
    fields: BTreeSet<Field>,
    edits: Vec<Edit>,
    session: String,
    meta: TxnMeta,
    at: Instant,
    revision: u64,
    submit: Option<(ara_collab::ParticipantId, u64, u64)>,
}

#[derive(Default)]
pub(crate) struct Gestures {
    fields: BTreeMap<Field, BTreeSet<String>>,
    branch_dirty: bool,
    owners: BTreeMap<String, ara_proto::ConnId>,
    held: VecDeque<Held>,
    receipts: BTreeMap<(String, String), BatchResult>,
    submits: BTreeMap<(String, u64), Value>,
}

/// Host service; editor UI code calls `editor.gesture` at input begin/end.
pub(crate) struct GestureService;
impl Service for GestureService {
    fn name(&self) -> &'static str {
        "gesture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<GestureParams>(db, "editor.gesture", "Begin or end an active human field gesture; agent ops on this exact field wait at most two seconds.").scope(ara_proto::Scope::EditScene)]
    }
    fn call(&mut self, cx: &mut Cx<'_>, _: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
        let p: GestureParams = parse(params)?;
        if cx.session().is_none_or(|s| s.author != Author::Human) {
            return Err(vec![Diagnostic::error(
                codes::MISSING_SCOPE,
                "only human editors own gestures",
            )]);
        }
        let doc = crate::core::find_doc(cx.st, &p.doc)?;
        let (entity, path) = match p.entity.split_once('/') {
            Some((root, inner)) => (
                root.to_owned(),
                ara_schema::FieldPath::field("overrides")
                    .key(format!("{inner}/{}", p.path))
                    .to_string(),
            ),
            None => (p.entity, p.path),
        };
        let _: ara_schema::FieldPath = path
            .parse::<ara_schema::FieldPath>()
            .map_err(|e| vec![Diagnostic::error(codes::INVALID_PARAMS, e.to_string())])?;
        let field = (doc.to_string(), entity, path);
        cx.st.gestures.branch_dirty = true;
        if p.active {
            let conn = cx.conn();
            cx.st.gestures.owners.insert(p.session.clone(), conn);
            cx.st
                .gestures
                .fields
                .entry(field)
                .or_default()
                .insert(p.session);
        } else if let Some(owners) = cx.st.gestures.fields.get_mut(&field) {
            owners.remove(&p.session);
            if owners.is_empty() {
                cx.st.gestures.fields.remove(&field);
            }
        }
        Ok(json!({"active":p.active}))
    }
}

fn fields(op: &Op) -> BTreeSet<Field> {
    op.touches()
        .into_iter()
        .map(|p| {
            (
                op.doc().to_string(),
                op.id().to_string(),
                p.map(|p| p.to_string()).unwrap_or_default(),
            )
        })
        .collect()
}

fn overlaps(a: &Field, b: &Field) -> bool {
    a.0 == b.0
        && a.1 == b.1
        && (a.2.is_empty()
            || b.2.is_empty()
            || a.2 == b.2
            || a.2
                .parse::<ara_schema::FieldPath>()
                .ok()
                .zip(b.2.parse::<ara_schema::FieldPath>().ok())
                .is_some_and(|(a, b)| a.overlaps(&b)))
}

impl State {
    /// Stages commands only while a human gesture could conflict. The normal path stays atomic.
    pub(crate) fn gesture_apply(
        &mut self,
        sid: &str,
        batch: &Batch,
        session: &ara_cmd::Session,
    ) -> Result<Option<BatchResult>, Vec<Diagnostic>> {
        if batch.dry_run || !matches!(session.author, Author::Agent(_)) {
            return Ok(None);
        }
        if let Some(key) = &batch.idempotency_key
            && let Some(receipt) = self.gestures.receipts.get(&(sid.to_owned(), key.clone()))
        {
            let mut receipt = receipt.clone();
            receipt.replayed = true;
            return Ok(Some(receipt));
        }
        if self.gestures.fields.is_empty() && self.gestures.held.is_empty() {
            return Ok(None);
        }
        let mut dry = batch.clone();
        dry.dry_run = true;
        let mut result = self.registry.apply(&mut self.store, &dry, session)?;
        let meta = TxnMeta {
            author: session.author.clone(),
            why: batch.why.clone(),
            label: Some(format!(
                "gesture-turn:{}|{}",
                self.token("turn"),
                batch
                    .calls
                    .iter()
                    .map(|c| c.command.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            base: None,
            idempotency_key: batch.idempotency_key.clone(),
        };
        let Some(held) = self.hold_ops(sid, meta, &result.ops, None)? else {
            return Ok(None);
        };
        result.txn = held.txn;
        result.dry_run = false;
        result.revision = self.store.revision();
        if let Some(key) = &batch.idempotency_key {
            self.gestures
                .receipts
                .insert((sid.to_owned(), key.clone()), result.clone());
        }
        Ok(Some(result))
    }

    fn hold_ops(
        &mut self,
        sid: &str,
        meta: TxnMeta,
        ops: &[Op],
        submit: Option<(ara_collab::ParticipantId, u64, u64)>,
    ) -> Result<Option<HeldResult>, Vec<Diagnostic>> {
        let mut held = Vec::new();
        let mut immediate = Vec::new();
        let mut held_fields = BTreeSet::new();
        for op in ops {
            let touched = fields(op);
            let waits = touched.iter().any(|f| {
                self.gestures.fields.keys().any(|g| overlaps(f, g))
                    || self
                        .gestures
                        .held
                        .iter()
                        .any(|h| h.fields.iter().any(|g| overlaps(f, g)))
            });
            if waits {
                held_fields.extend(touched);
                held.push(op.to_edit());
            } else {
                immediate.push(op.to_edit());
            }
        }
        if held.is_empty() {
            return Ok(None);
        }
        let count = held.len();
        let revision = self.store.revision();
        let txn = self
            .store
            .transact(meta.clone(), immediate)?
            .txn()
            .map(|t| t.id.clone());
        if let (Some(txn), Some((participant, seq, base))) = (&txn, &submit) {
            self.collab
                .hub
                .attribute(txn.clone(), participant.clone(), Some(*seq), *base);
        }
        let mut meta = meta;
        if txn.is_some() {
            meta.idempotency_key = Some(self.token("gesture"));
        }
        self.gestures.held.push_back(Held {
            fields: held_fields,
            edits: held,
            session: sid.to_owned(),
            meta,
            at: Instant::now(),
            revision,
            submit: if txn.is_none() { submit } else { None },
        });
        Ok(Some(HeldResult { txn, count }))
    }

    pub(crate) fn gesture_submit(
        &mut self,
        sid: &str,
        submit: &ara_collab::Submit,
    ) -> Result<Option<Value>, Vec<Diagnostic>> {
        let Some(session) = self.sessions.live(sid).cloned() else {
            return Ok(None);
        };
        if !matches!(session.author, Author::Agent(_)) {
            return Ok(None);
        }
        if let Some(result) = self.gestures.submits.get(&(sid.to_owned(), submit.seq)) {
            let mut result = result.clone();
            result["replayed"] = true.into();
            return Ok(Some(result));
        }
        if self.gestures.fields.is_empty() && self.gestures.held.is_empty() {
            return Ok(None);
        }
        if self
            .store
            .find_by_key(&format!("{sid}#{}", submit.seq))
            .is_some()
        {
            return Ok(None);
        }
        let meta = TxnMeta {
            author: session.author,
            why: submit.why.clone(),
            label: Some(format!(
                "gesture-turn:{}|{}",
                self.token("turn"),
                submit.label.as_deref().unwrap_or("collab.submit")
            )),
            base: None,
            idempotency_key: Some(format!("{sid}#{}", submit.seq)),
        };
        let mut staged = self.store.begin(meta.clone());
        for edit in &submit.edits {
            staged.apply(edit.clone()).map_err(|d| vec![d])?;
        }
        let errors = staged.check();
        if !errors.is_empty() {
            return Err(errors);
        }
        let ops = staged.rollback();
        let origin = Some((
            ara_collab::ParticipantId(sid.to_owned()),
            submit.seq,
            submit.base,
        ));
        let Some(held) = self.hold_ops(sid, meta, &ops, origin)? else {
            return Ok(None);
        };
        let value = json!({"txn":held.txn,"revision":self.store.revision(),"replayed":false,"overwrote":0,"gestureHeld":held.count});
        self.gestures
            .submits
            .insert((sid.to_owned(), submit.seq), value.clone());
        Ok(Some(value))
    }

    pub(crate) fn release_gestures(&mut self) {
        if self.gestures.fields.is_empty()
            && self.gestures.held.is_empty()
            && !self.gestures.branch_dirty
        {
            return;
        }
        let before = self.gestures.fields.len();
        let sessions = &self.sessions;
        self.gestures.fields.retain(|_, owners| {
            owners.retain(|sid| sessions.live(sid).is_some());
            !owners.is_empty()
        });
        self.gestures.branch_dirty |= before != self.gestures.fields.len();
        let mut remaining = VecDeque::new();
        while let Some(held) = self.gestures.held.pop_front() {
            let waits = held.fields.iter().any(|f| {
                self.gestures.fields.keys().any(|g| overlaps(f, g))
                    || remaining
                        .iter()
                        .any(|h: &Held| h.fields.iter().any(|g| overlaps(f, g)))
            });
            if waits && held.at.elapsed() < Duration::from_secs(2) {
                remaining.push_back(held);
                continue;
            }
            let calling = self.calling.replace(held.session.clone());
            self.store.set_origin(Some(held.session));
            match self.store.transact(held.meta, held.edits) {
                Ok(commit) => {
                    if let Some(txn) = commit.txn().map(|t| t.id.clone())
                        && let Some((participant, seq, base)) = held.submit
                    {
                        self.collab.hub.attribute(txn, participant, Some(seq), base);
                    }
                }
                Err(diags) => {
                    for d in diags {
                        self.log(d);
                    }
                }
            }
            self.drain();
            self.calling = calling;
            self.store.set_origin(self.calling.clone());
        }
        self.gestures.held = remaining;
        self.refresh_branch_gestures(crate::collab::now_ms(), false);
        self.collab
            .hub
            .tick(&mut self.store, crate::collab::now_ms());
        self.drain();
    }

    pub(crate) fn refresh_branch_gestures(&mut self, now: u64, force: bool) {
        if !force && !std::mem::take(&mut self.gestures.branch_dirty) {
            return;
        }
        let branches: Vec<_> = self
            .collab
            .hub
            .branches()
            .filter(|b| {
                matches!(
                    b.status,
                    ara_collab::BranchStatus::Open | ara_collab::BranchStatus::Submitted
                )
            })
            .map(|branch| {
                let active = branch
                    .store()
                    .history()
                    .iter()
                    .flat_map(|t| t.ops.iter())
                    .flat_map(fields)
                    .any(|f| self.gestures.fields.keys().any(|g| overlaps(&f, g)));
                (branch.id.clone(), active)
            })
            .collect();
        for (branch, active) in branches {
            self.collab.hub.branch_gesture(&branch, active, now);
        }
    }

    pub(crate) fn held_count(&self, sid: &str) -> usize {
        self.gestures
            .held
            .iter()
            .filter(|h| h.session == sid)
            .map(|h| h.edits.len())
            .sum()
    }

    pub(crate) fn cancel_held_from(&mut self, sid: &str, revision: u64) {
        self.gestures
            .held
            .retain(|held| held.session != sid || held.revision < revision);
    }

    pub(crate) fn cancel_held_turn(&mut self, group: &str) {
        self.gestures
            .held
            .retain(|held| turn(held.meta.label.as_deref()).is_none_or(|(id, _)| id != group));
    }

    pub(crate) fn close_gestures(&mut self, conn: ara_proto::ConnId) {
        self.gestures.branch_dirty = true;
        self.gestures.owners.retain(|_, owner| *owner != conn);
        let owners = &self.gestures.owners;
        self.gestures.fields.retain(|_, sessions| {
            sessions.retain(|sid| owners.contains_key(sid));
            !sessions.is_empty()
        });
        self.release_gestures();
    }
}
