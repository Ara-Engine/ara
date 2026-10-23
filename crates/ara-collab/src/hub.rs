//! [`Hub`]: the authority's collaboration state next to the live `Store`: who is
//! here, presence, claims, the conflict log, per-participant undo, branch sessions,
//! and the outbox of notifications for participants.
//!
//! The hub never does IO and never reads a clock: every method takes `now` (ms) and
//! the store it works on. The project host calls it from its `collab` service and
//! after every call ([`publish`](Hub::publish)); tests drive it directly.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ara_diag::{Diagnostic, codes as kernel};
use ara_doc::IdGen;
use ara_ops::{Author, Store, Transaction, TxnId, TxnMeta};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::branch::Branch;
use crate::claims::{Claim, Claims, Granted, Wanted};
use crate::participant::{Kind, Participant, ParticipantId, Roster};
use crate::presence::{Activity, Presence, PresenceBoard};
use crate::sync::{
    Conflict, ConflictKind, ConflictLog, OpEvent, Reject, Side, Submit, agree, methods, overlap,
    path_of, written,
};
use crate::{codes, who};

/// Who a notification goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Every joined participant.
    All,
    /// One participant.
    Only(ParticipantId),
}

/// A notification for participants: JSON-RPC `method` with `params`.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Recipients.
    pub to: Audience,
    /// Notification method (see [`methods`]).
    pub method: &'static str,
    /// Its params.
    pub params: Value,
}

/// When submitted branch sessions merge into the live documents (D20). Mirrors the
/// editor's agent apply policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplyPolicy {
    /// As soon as nobody claims what the branch touches.
    #[default]
    Immediately,
    /// Once no human has edited or moved for `idle_ms`.
    WhenIdle,
    /// Only when a human approves (`branch.merge`).
    OnApproval,
}

/// Causal metadata of one transaction.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Origin {
    pub(crate) participant: Option<ParticipantId>,
    pub(crate) seq: Option<u64>,
    pub(crate) base: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Stacks {
    pub(crate) undo: Vec<TxnId>,
    pub(crate) redo: Vec<TxnId>,
}

/// What `collab.submit` returns when the transaction committed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    /// The committed transaction.
    pub txn: TxnId,
    /// Its authority revision.
    pub revision: u64,
    /// True if this sequence number was already applied (a resubmit).
    pub replayed: bool,
    /// Concurrent changes by others this one overwrote (logged for them).
    pub overwrote: usize,
}

/// The authority's collaboration state.
#[derive(Debug)]
pub struct Hub {
    pub(crate) roster: Roster,
    pub(crate) presence: PresenceBoard,
    pub(crate) claims: Claims,
    pub(crate) conflicts: ConflictLog,
    pub(crate) branches: BTreeMap<String, Branch>,
    /// Branches changed since the last `take_dirty_branches`.
    pub(crate) dirty: BTreeSet<String>,
    pub(crate) origins: HashMap<TxnId, Origin>,
    pub(crate) stacks: BTreeMap<ParticipantId, Stacks>,
    pub(crate) published: u64,
    pub(crate) events: Vec<Event>,
    pub(crate) ids: IdGen,
    pub(crate) policy: ApplyPolicy,
    pub(crate) idle_ms: u64,
    pub(crate) last_human: u64,
    /// Bumped when claims are taken, released or expire.
    pub(crate) claims_version: u64,
    /// When the claim guard was last installed (ms).
    pub(crate) guard_at: u64,
}

/// Presence broadcasts per participant are at most one per this interval (20 Hz).
pub const PRESENCE_INTERVAL_MS: u64 = 50;

impl Hub {
    /// A hub over a store at `revision` (transactions up to it are not broadcast).
    pub fn new(revision: u64, ids: IdGen) -> Hub {
        Hub {
            roster: Roster::default(),
            presence: PresenceBoard::new(PRESENCE_INTERVAL_MS),
            claims: Claims::default(),
            conflicts: ConflictLog::new(256),
            branches: BTreeMap::new(),
            dirty: BTreeSet::new(),
            origins: HashMap::new(),
            stacks: BTreeMap::new(),
            published: revision,
            events: Vec::new(),
            ids,
            policy: ApplyPolicy::default(),
            idle_ms: 2_000,
            last_human: 0,
            claims_version: 0,
            guard_at: 0,
        }
    }

    /// Notifications waiting to go out, oldest first.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn emit(&mut self, to: Audience, method: &'static str, params: Value) {
        self.events.push(Event { to, method, params });
    }

    /// Everyone here.
    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    /// Presence of everyone here.
    pub fn presence(&self) -> &PresenceBoard {
        &self.presence
    }

    /// Live claims.
    pub fn claims(&self) -> &[Claim] {
        self.claims.all()
    }

    /// Changes whenever claims are taken, released or expire (not on heartbeats):
    /// persist then.
    pub fn claims_version(&self) -> u64 {
        self.claims_version
    }

    /// The conflict log.
    pub fn conflicts(&self) -> &ConflictLog {
        &self.conflicts
    }

    /// The branch apply policy and its idle threshold.
    pub fn policy(&self) -> (ApplyPolicy, u64) {
        (self.policy, self.idle_ms)
    }

    /// Sets the branch apply policy (humans decide; the host checks).
    pub fn set_policy(&mut self, policy: ApplyPolicy, idle_ms: Option<u64>) {
        self.policy = policy;
        if let Some(ms) = idle_ms {
            self.idle_ms = ms;
        }
    }

    /// The participant, or `ARA2502`.
    pub fn participant(&self, id: &ParticipantId) -> Result<&Participant, Vec<Diagnostic>> {
        self.roster.get(id).ok_or_else(|| vec![not_joined(id)])
    }

    fn roster_event(&mut self) {
        let participants: Vec<&Participant> = self.roster.all().collect();
        let params = json!({ "participants": participants });
        self.emit(Audience::All, methods::ROSTER, params);
    }

    /// Installs the claim guard as of `now`.
    fn install(&mut self, store: &mut Store, now: u64) {
        store.set_guard(Some(Box::new(self.claims.guard(now))));
        self.guard_at = now;
    }

    fn claims_changed(&mut self, store: &mut Store, now: u64) {
        self.install(store, now);
        self.claims_version += 1;
        let params = json!({ "claims": self.claims.all() });
        self.emit(Audience::All, methods::CLAIMS, params);
    }

    /// Joins (or re-joins) `id` as `author`. Returns the participant.
    pub fn join(
        &mut self,
        id: ParticipantId,
        author: Author,
        name: Option<String>,
        color: Option<String>,
        now: u64,
    ) -> Participant {
        let p = self.roster.join(id, author, name, color, now).clone();
        if p.kind == Kind::Human {
            self.last_human = now;
        }
        self.roster_event();
        p
    }

    /// `id` leaves: its claims are released and its presence forgotten. Branch
    /// sessions stay (they are reviewable work).
    pub fn leave(&mut self, store: &mut Store, id: &ParticipantId, now: u64) {
        let released = self.claims.release(id, None);
        self.presence.remove(id);
        if self.roster.leave(id).is_some() {
            self.roster_event();
        }
        if !released.is_empty() {
            self.claims_changed(store, now);
        }
    }

    /// `id`'s connection dropped: presence and roster entry go, claims stay until
    /// they expire without heartbeats (a reconnect keeps them).
    pub fn disconnect(&mut self, id: &ParticipantId) {
        self.presence.remove(id);
        if self.roster.leave(id).is_some() {
            self.roster_event();
        }
    }

    /// Stores `id`'s presence (rate-limited broadcast). Presence is a heartbeat: it
    /// extends `id`'s claims.
    pub fn set_presence(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
        mut presence: Presence,
        now: u64,
    ) -> Result<(), Vec<Diagnostic>> {
        let kind = self.participant(id)?.kind;
        if let Some(a) = &mut presence.activity {
            a.at.get_or_insert(now);
        }
        if kind == Kind::Human {
            self.last_human = now;
        }
        if let Some(p) = self.presence.set(id, presence, now) {
            self.emit(
                Audience::All,
                methods::PRESENCE,
                json!({ "participant": id, "presence": p }),
            );
        }
        self.heartbeat(store, id, now);
        Ok(())
    }

    /// Claims `wanted` for `id` (see [`Claims::request`]).
    pub fn claim(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
        wanted: Vec<Wanted>,
        ttl: u64,
        why: Option<String>,
        now: u64,
    ) -> Result<Granted, Vec<Diagnostic>> {
        let holder = self.participant(id)?.clone();
        self.expire(store, now);
        let granted = self
            .claims
            .request(&holder, wanted, ttl, why, now, &mut self.ids)?;
        self.claims_changed(store, now);
        Ok(granted)
    }

    /// Releases `id`'s claims with these IDs (all for `None`).
    pub fn release(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
        ids: Option<&[String]>,
        now: u64,
    ) -> Vec<Claim> {
        let gone = self.claims.release(id, ids);
        if !gone.is_empty() {
            self.claims_changed(store, now);
        }
        gone
    }

    /// Extends `id`'s claims.
    pub fn heartbeat(&mut self, store: &mut Store, id: &ParticipantId, now: u64) {
        if self.claims.heartbeat(id, now) > 0 {
            self.install(store, now);
        }
    }

    /// Drops ended claims.
    pub fn expire(&mut self, store: &mut Store, now: u64) {
        if !self.claims.expire(now).is_empty() {
            self.claims_changed(store, now);
        } else if now.saturating_sub(self.guard_at) >= 1000 {
            // Keeps the "time left" in ARA2500 messages within a second.
            self.install(store, now);
        }
    }

    /// Restores persisted claims (dropping ended ones) and installs their guard.
    pub fn restore_claims(&mut self, store: &mut Store, claims: Vec<Claim>, now: u64) {
        self.claims.restore(claims);
        self.claims.expire(now);
        self.install(store, now);
    }

    /// Periodic work: claim expiry, coalesced presence, branch merges per policy.
    pub fn tick(&mut self, store: &mut Store, now: u64) {
        self.expire(store, now);
        for (id, p) in self.presence.flush(now) {
            self.emit(
                Audience::All,
                methods::PRESENCE,
                json!({ "participant": id, "presence": p }),
            );
        }
        self.merge_due(store, now);
    }

    /// The participant behind a transaction, if known.
    pub fn owner(&self, txn: &TxnId) -> Option<&ParticipantId> {
        self.origins.get(txn).and_then(|o| o.participant.as_ref())
    }

    fn name_of(&self, who: Option<&ParticipantId>, t: &Transaction) -> String {
        who.map_or_else(|| t.author.to_string(), |p| self.roster.name(p))
    }

    /// Applies a replica's change on top of everything since its base: field-level,
    /// last writer (this one) wins, overwritten concurrent changes are logged for
    /// their authors. If it no longer applies, it is dropped whole: logged, answered
    /// with a `collab.reject` to `id`, and returned as the error.
    pub fn submit(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
        s: Submit,
        now: u64,
    ) -> Result<Ack, Vec<Diagnostic>> {
        let author = self.participant(id)?.author.clone();
        if Kind::of(&author) == Kind::Human {
            self.last_human = now;
        }
        let key = format!("{id}#{}", s.seq);
        if let Some(t) = store.find_by_key(&key) {
            return Ok(Ack {
                txn: t.id.clone(),
                revision: t.result,
                replayed: true,
                overwrote: 0,
            });
        }
        let base = s.base.min(store.revision());
        let concurrent: Vec<Transaction> = store
            .since(base)
            .iter()
            .filter(|t| self.owner(&t.id) != Some(id))
            .cloned()
            .collect();
        let first = s.edits.first().cloned();
        let meta = TxnMeta {
            author,
            why: s.why.clone(),
            label: Some(s.label.clone().unwrap_or_else(|| "collab.submit".into())),
            base: None,
            idempotency_key: Some(key),
        };
        let result = if s.edits.is_empty() {
            Err(vec![Diagnostic::error(
                kernel::INVALID_PARAMS,
                "a submit needs at least one edit",
            )])
        } else {
            store.transact(meta, s.edits).map(|c| c.txn().cloned())
        };
        let txn = match result {
            Ok(Some(t)) => t,
            other => {
                // A non-empty edit list always records a transaction; `None` only
                // guards the type.
                let diags = other.err().unwrap_or_else(|| {
                    vec![Diagnostic::error(
                        kernel::INVALID_PARAMS,
                        "the submit recorded nothing",
                    )]
                });
                self.dropped(store, id, &s.seq, first.as_ref(), &concurrent, &diags);
                return Err(diags);
            }
        };
        self.origins.insert(
            txn.id.clone(),
            Origin {
                participant: Some(id.clone()),
                seq: Some(s.seq),
                base,
            },
        );
        let overwrote = self.overwritten(id, &txn, &concurrent);
        Ok(Ack {
            txn: txn.id.clone(),
            revision: txn.result,
            replayed: false,
            overwrote,
        })
    }

    /// Logs the concurrent changes `txn` replaced; returns how many.
    fn overwritten(
        &mut self,
        id: &ParticipantId,
        txn: &Transaction,
        concurrent: &[Transaction],
    ) -> usize {
        let winner_name = self.roster.name(id);
        let mut logged: Vec<(TxnId, String)> = Vec::new();
        let mut entries = Vec::new();
        for theirs in concurrent {
            let loser = self.owner(&theirs.id).cloned();
            for lost in &theirs.ops {
                for mine in txn.ops.iter().filter(|m| overlap(m, lost)) {
                    if agree(mine, lost) {
                        continue;
                    }
                    let key = (
                        theirs.id.clone(),
                        format!("{}{:?}", lost.id(), path_of(lost)),
                    );
                    if logged.contains(&key) {
                        continue;
                    }
                    logged.push(key);
                    let what = path_of(lost).map_or_else(
                        || format!("`{}`", lost.id()),
                        |p| format!("`{}.{p}`", lost.id()),
                    );
                    entries.push(Conflict {
                        id: 0,
                        kind: ConflictKind::Overwritten,
                        revision: txn.result,
                        doc: lost.doc().clone(),
                        record: Some(lost.id().clone()),
                        path: path_of(lost),
                        loser: Side {
                            name: self.name_of(loser.as_ref(), theirs),
                            participant: loser.clone(),
                            txn: Some(theirs.id.clone()),
                            seq: self.origins.get(&theirs.id).and_then(|o| o.seq),
                            value: written(lost),
                        },
                        winner: Some(Side {
                            participant: Some(id.clone()),
                            name: winner_name.clone(),
                            txn: Some(txn.id.clone()),
                            seq: self.origins.get(&txn.id).and_then(|o| o.seq),
                            value: written(mine),
                        }),
                        message: format!(
                            "your change to {what} was overwritten by {winner_name}'s concurrent change (last writer wins); yours is kept in the conflict log"
                        ),
                    });
                }
            }
        }
        let n = entries.len();
        for c in entries {
            self.log_conflict(c);
        }
        n
    }

    /// Logs and reports a dropped submit.
    fn dropped(
        &mut self,
        store: &Store,
        id: &ParticipantId,
        seq: &u64,
        first: Option<&ara_doc::Edit>,
        concurrent: &[Transaction],
        diags: &[Diagnostic],
    ) {
        let record = diags
            .first()
            .and_then(|d| d.location.record.clone())
            .and_then(|r| ara_doc::RecordId::parse(&r).ok());
        let doc = first.map(|e| e.doc().clone());
        let cause = concurrent.iter().rev().find(|t| {
            t.ops
                .iter()
                .any(|op| Some(op.id()) == record.as_ref() && Some(op.doc()) == doc.as_ref())
        });
        let why = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        if let Some(doc) = doc {
            let winner = cause.map(|t| {
                let who = self.owner(&t.id).cloned();
                Side {
                    name: self.name_of(who.as_ref(), t),
                    participant: who,
                    txn: Some(t.id.clone()),
                    seq: None,
                    value: None,
                }
            });
            let by = winner
                .as_ref()
                .map(|w| format!(" after {}'s concurrent change", w.name))
                .unwrap_or_default();
            self.log_conflict(Conflict {
                id: 0,
                kind: ConflictKind::Dropped,
                revision: store.revision(),
                doc,
                record,
                path: None,
                loser: Side {
                    participant: Some(id.clone()),
                    name: self.roster.name(id),
                    txn: None,
                    seq: Some(*seq),
                    value: None,
                },
                winner,
                message: format!("your change was dropped{by}: {why}"),
            });
        }
        let reject = Reject {
            seq: *seq,
            diagnostics: diags.to_vec(),
        };
        self.emit(
            Audience::Only(id.clone()),
            methods::REJECT,
            serde_json::to_value(reject).unwrap_or(Value::Null),
        );
    }

    fn log_conflict(&mut self, c: Conflict) {
        let c = self.conflicts.push(c);
        if let Some(loser) = c.loser.participant.clone() {
            let params = serde_json::to_value(&c).unwrap_or(Value::Null);
            self.emit(Audience::Only(loser), methods::CONFLICT, params);
        }
    }

    /// Broadcasts every transaction committed since the last call, in order, as
    /// `collab.op` with its causal metadata. `attribution` names the participant
    /// (session) of transactions the hub did not commit itself. Also feeds
    /// per-participant undo stacks and agents' activity.
    pub fn publish(
        &mut self,
        store: &Store,
        attribution: &dyn Fn(&TxnId) -> Option<ParticipantId>,
        now: u64,
    ) {
        for t in store.since(self.published) {
            let origin = self.origins.get(&t.id).cloned().unwrap_or_else(|| Origin {
                participant: attribution(&t.id),
                seq: None,
                base: t.base,
            });
            self.origins.insert(t.id.clone(), origin.clone());
            if let Some(p) = &origin.participant
                && t.reverts.is_none()
            {
                let stacks = self.stacks.entry(p.clone()).or_default();
                stacks.undo.push(t.id.clone());
                stacks.redo.clear();
                if self.roster.get(p).is_some_and(|x| x.kind == Kind::Human) {
                    self.last_human = now;
                }
                let activity = Activity {
                    what: t.label.clone().unwrap_or_else(|| "edit".into()),
                    why: t.why.clone(),
                    at: Some(now),
                };
                if self.roster.get(p).is_some()
                    && let Some(presence) = self.presence.set_activity(p, activity, now)
                {
                    self.emit(
                        Audience::All,
                        methods::PRESENCE,
                        json!({ "participant": p, "presence": presence }),
                    );
                }
            }
            let event = OpEvent {
                participant: origin.participant,
                seq: origin.seq,
                base: origin.base,
                txn: t.clone(),
            };
            self.emit(
                Audience::All,
                methods::OP,
                serde_json::to_value(event).unwrap_or(Value::Null),
            );
            self.published = t.result;
        }
    }

    /// Every transaction after `since` as op events: a late joiner or a reconnecting
    /// replica replays these on top of its snapshot.
    pub fn replay(&self, store: &Store, since: u64) -> Vec<OpEvent> {
        store
            .since(since)
            .iter()
            .map(|t| {
                let o = self.origins.get(&t.id);
                OpEvent {
                    participant: o.and_then(|o| o.participant.clone()),
                    seq: o.and_then(|o| o.seq),
                    base: o.map_or(t.base, |o| o.base),
                    txn: t.clone(),
                }
            })
            .collect()
    }

    /// The token-efficient "who is doing what" text for `me` (see [`who::summary`]).
    pub fn who(&self, store: &Store, me: Option<&ParticipantId>, now: u64) -> String {
        who::summary(self, store, me, now)
    }
}

/// `ARA2502` with the fix: join.
pub fn not_joined(id: &ParticipantId) -> Diagnostic {
    Diagnostic::error(
        codes::NOT_JOINED,
        format!("session `{id}` has not joined collaboration; call `collab.join` first"),
    )
    .with_fix(
        "Join",
        ara_diag::FixAction::Call {
            method: "collab.join".into(),
            params: json!({}),
        },
    )
}
