//! Collaboration over the Engine Protocol (D21): the `collab` service wrapping an
//! `ara_collab::Hub` next to the live store.
//!
//! | Methods | MCP |
//! |---|---|
//! | `collab.who`, `collab.claim`, `collab.release`, `branch.open`, `branch.submit` | `ara_session` actions `who`, `claim`, `release`, `branch`, `submit` |
//! | `collab.join/leave/presence/heartbeat/claims/submit/sync/undo/redo/conflicts/policy`, `branch.list/status/merge/discard` | none (editors, replicas) |
//!
//! - Participants are sessions. `collab.join` subscribes the connection to the
//!   `collab.*` notifications; every other collab method joins the session on first
//!   use (agents never need to call `join`).
//! - After every call the host publishes new transactions as `collab.op` with
//!   causal metadata, routes the hub's events to joined connections and persists
//!   claims (`.ara/claims.json`) and branch sessions (`.ara/branches/`). Presence
//!   is never persisted.
//! - The hub installs the claim guard on the store; the host sets the store's
//!   origin to the calling session, so agents are blocked by everyone's claims but
//!   their own.
//! - `cmd.apply` with `branch` runs in a branch session ([`branch_apply`]).

mod params;
mod subject;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ara_collab::claims::{Claim, DEFAULT_TTL_MS};
use ara_collab::codes;
use ara_collab::{
    ApplyPolicy, Audience, Hub, Kind, ParticipantId, Prefer, Presence, SavedBranch, Snapshot,
    Submit,
};
use ara_diag::{Diagnostic, codes as kernel};
use ara_doc::{Edit, IdGen};
use ara_ops::Author;
use ara_proto::{ConnId, Scope, jsonrpc};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

pub use params::*;

use crate::core::{apply_text, diff_summary, relocate};
use crate::host::{State, session_not_found};
use crate::params::ApplyParams;
use crate::service::{Cx, Method, Service, parse};

type R = Result<Value, Vec<Diagnostic>>;

/// Milliseconds since the Unix epoch: claims persist with absolute expiry times.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The host's collaboration state.
pub(crate) struct Collab {
    pub(crate) hub: Hub,
    /// Connections that joined, per participant (notification routing).
    conns: BTreeMap<ParticipantId, ConnId>,
    /// `.ara`.
    dir: PathBuf,
    /// The claims version last written to `.ara/claims.json`, and when (ms).
    saved_claims: (u64, u64),
}

impl Collab {
    /// A placeholder until [`State::collab_open`] runs after journal recovery.
    pub(crate) fn new(dir: PathBuf) -> Collab {
        Collab {
            hub: Hub::new(0, IdGen::seeded(0)),
            conns: BTreeMap::new(),
            dir,
            saved_claims: (0, 0),
        }
    }
}

fn pid(sid: &str) -> ParticipantId {
    ParticipantId(sid.to_owned())
}

impl State {
    /// Starts collaboration after recovery: broadcasting begins at the current
    /// revision; persisted claims (of live sessions) and branch sessions come back.
    pub(crate) fn collab_open(&mut self, seed: Option<u64>) {
        let ids = seed.map_or_else(IdGen::from_entropy, |s| IdGen::seeded(s ^ 0xc011_ab00));
        self.collab.hub = Hub::new(self.store.revision(), ids);
        let now = now_ms();
        let claims_file = self.collab.dir.join("claims.json");
        if let Ok(text) = fs::read_to_string(&claims_file) {
            let claims: Vec<Claim> = serde_json::from_str(&text).unwrap_or_default();
            let live: Vec<Claim> = claims
                .into_iter()
                .filter(|c| self.sessions.live(&c.holder.0).is_some())
                .collect();
            self.collab.hub.restore_claims(&mut self.store, live, now);
        } else {
            self.collab
                .hub
                .restore_claims(&mut self.store, Vec::new(), now);
        }
        let dir = self.collab.dir.join("branches");
        let mut problems = Vec::new();
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let loaded = fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<SavedBranch>(&t).ok());
            let restored = loaded.map(|b| self.collab.hub.restore_branch(b, &self.store));
            if !matches!(restored, Some(Ok(()))) {
                problems.push(path.display().to_string());
            }
        }
        for p in problems {
            self.diag.push(
                Diagnostic::warning(
                    kernel::IO,
                    format!("branch session file `{p}` could not be restored; it was skipped"),
                ),
                ara_diag::bus::Meta::default(),
            );
        }
    }

    /// After every call and tick: ended sessions leave, new transactions are
    /// published, events are routed, claims and branches persisted.
    pub(crate) fn collab_after(&mut self) {
        let now = now_ms();
        let ended: BTreeSet<ParticipantId> = self
            .collab
            .hub
            .roster()
            .all()
            .map(|p| p.id.clone())
            .chain(self.collab.hub.claims().iter().map(|c| c.holder.clone()))
            .filter(|id| self.sessions.live(&id.0).is_none())
            .collect();
        for id in ended {
            self.collab.hub.leave(&mut self.store, &id, now);
            self.collab.conns.remove(&id);
        }
        let sessions = &self.txn_session;
        self.collab
            .hub
            .publish(&self.store, &|t| sessions.get(t).map(|s| pid(s)), now);
        self.collab_flush();
    }

    /// Routes hub events to joined connections and persists what changed.
    fn collab_flush(&mut self) {
        for e in self.collab.hub.drain_events() {
            let msg = jsonrpc::notification(e.method, e.params);
            let to: BTreeSet<ConnId> = match &e.to {
                Audience::All => self.collab.conns.values().copied().collect(),
                Audience::Only(p) => self.collab.conns.get(p).copied().into_iter().collect(),
            };
            for conn in to {
                self.outbox.push((conn, msg.clone()));
            }
        }
        // Heartbeats only move expiry times: write those at most every 10 s.
        let (version, now) = (self.collab.hub.claims_version(), now_ms());
        let (saved, at) = self.collab.saved_claims;
        let stale = !self.collab.hub.claims().is_empty() && now.saturating_sub(at) >= 10_000;
        if version != saved || stale {
            let claims = serde_json::to_string_pretty(self.collab.hub.claims()).unwrap_or_default();
            let path = self.collab.dir.join("claims.json");
            match fs::write(&path, claims) {
                Ok(()) => self.collab.saved_claims = (version, now),
                Err(e) => self.log(Diagnostic::warning(
                    kernel::IO,
                    format!("cannot write `.ara/claims.json`: {e}"),
                )),
            }
        }
        let dirty = self.collab.hub.take_dirty_branches();
        if !dirty.is_empty() {
            let dir = self.collab.dir.join("branches");
            let result = fs::create_dir_all(&dir).and_then(|()| {
                dirty.iter().try_for_each(|b| {
                    let text = serde_json::to_string(b).unwrap_or_default();
                    fs::write(dir.join(format!("{}.json", b.id)), text)
                })
            });
            if let Err(e) = result {
                self.log(Diagnostic::warning(
                    kernel::IO,
                    format!("cannot write `.ara/branches/`: {e}"),
                ));
            }
        }
    }

    /// A connection closed: its participants disconnect (presence goes; claims stay
    /// until they expire without heartbeats).
    pub(crate) fn collab_closed(&mut self, conn: ConnId) {
        let gone: Vec<ParticipantId> = self
            .collab
            .conns
            .iter()
            .filter(|(_, c)| **c == conn)
            .map(|(p, _)| p.clone())
            .collect();
        for p in gone {
            self.collab.conns.remove(&p);
            self.collab.hub.disconnect(&p);
        }
        self.collab_flush();
    }

    /// Joins `sid` on first use, named after its author (made unique).
    fn ensure_joined(&mut self, sid: &str, name: Option<String>, color: Option<String>) -> R {
        let session = self
            .sessions
            .live(sid)
            .cloned()
            .ok_or_else(|| vec![session_not_found(sid)])?;
        let id = pid(sid);
        if name.is_none() && color.is_none() && self.collab.hub.roster().get(&id).is_some() {
            return Ok(Value::Null);
        }
        let base = name.unwrap_or_else(|| match &session.author {
            Author::Human => session.label.clone().unwrap_or_else(|| "human".into()),
            Author::Agent(n) => n.clone(),
        });
        let taken = |n: &str| {
            self.collab
                .hub
                .roster()
                .all()
                .any(|p| p.id != id && p.name == n)
        };
        let mut unique = base.clone();
        let mut i = 2;
        while taken(&unique) {
            unique = format!("{base}-{i}");
            i += 1;
        }
        let p = self
            .collab
            .hub
            .join(id, session.author, Some(unique), color, now_ms());
        Ok(serde_json::to_value(p).unwrap_or(Value::Null))
    }
}

/// The `collab` service.
pub(crate) struct CollabService;

/// The collaboration method table.
pub(crate) fn methods(db: &mut SchemaDb) -> Vec<Method> {
    let s = "ara_session";
    vec![
        Method::new::<JoinParams>(db, "collab.join", "Join collaboration: this connection gets presence, claims, op and conflict notifications; returns everyone's state (and a snapshot on request)."),
        Method::new::<crate::params::SessionParams>(db, "collab.leave", "Leave collaboration: releases your claims and clears your presence."),
        Method::new::<PresenceParams>(db, "collab.presence", "Set your presence (open document, camera, selection, pointer, activity, following); broadcast rate-limited. Also a heartbeat for your claims."),
        Method::new::<crate::params::SessionParams>(db, "collab.who", "Who is here and doing what: participants, what they look at and do, their claims (which agents must avoid) and branch sessions, as short text.")
            .read_only()
            .action(s, "who"),
        Method::new::<ClaimParams>(db, "collab.claim", "Claim entities, components, documents or files for your turn, with an expiry. Others' agents cannot edit what you claim; humans are only warned.")
            .scope(Scope::EditScene)
            .action(s, "claim"),
        Method::new::<ReleaseParams>(db, "collab.release", "Release your claims (all, or by ID).")
            .scope(Scope::EditScene)
            .action(s, "release"),
        Method::new::<crate::params::SessionParams>(db, "collab.heartbeat", "Extend your claims by their lifetime."),
        Method::new::<crate::params::SessionParams>(db, "collab.claims", "Every live claim, structured.")
            .read_only(),
        Method::new::<SubmitParams>(db, "collab.submit", "Submit a replica's optimistic edits: merged field-level onto everything since `base`; the same field is last-writer-wins and the loser is logged.")
            .scope(Scope::EditScene),
        Method::new::<SyncParams>(db, "collab.sync", "Op events after a revision (replay), or a snapshot of every document (late join, resync).")
            .read_only(),
        Method::new::<crate::params::SessionParams>(db, "collab.undo", "Undo your latest change without undoing anyone else's: fields others changed since keep their value.")
            .scope(Scope::EditScene),
        Method::new::<crate::params::SessionParams>(db, "collab.redo", "Redo your latest collab undo.")
            .scope(Scope::EditScene),
        Method::new::<ConflictsParams>(db, "collab.conflicts", "The conflict log: changes that lost (overwritten, dropped, kept-live in a merge), with both values.")
            .read_only(),
        Method::new::<PolicyParams>(db, "collab.policy", "When submitted agent branches merge: immediately, when the humans are idle, or on approval. Humans set it.")
            .scope(Scope::EditScene),
        Method::new::<BranchOpenParams>(db, "branch.open", "Open a branch session: a private copy of the live documents to apply your turn in (`ara_apply` with `branch`); nothing is live until it is submitted and merged.")
            .scope(Scope::EditScene)
            .action(s, "branch"),
        Method::new::<crate::params::SessionParams>(db, "branch.list", "Every branch session with its status and changes.")
            .read_only(),
        Method::new::<BranchParams>(db, "branch.status", "One branch session: status, what it changed (was → now), what it waits on, the merge report.")
            .read_only(),
        Method::new::<BranchSubmitParams>(db, "branch.submit", "Submit your branch session for merging; it merges per the human's apply policy once nobody claims what it touches.")
            .scope(Scope::EditScene)
            .action(s, "submit"),
        Method::new::<BranchMergeParams>(db, "branch.merge", "Approve and merge a branch session now (humans only): field-level three-way merge with a conflict report.")
            .scope(Scope::EditScene),
        Method::new::<BranchParams>(db, "branch.discard", "Throw a branch session away (its owner or a human).")
            .scope(Scope::EditScene)
            .destructive(),
    ]
}

impl Service for CollabService {
    fn name(&self) -> &'static str {
        "collab"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        methods(db)
    }

    fn call(&mut self, cx: &mut Cx<'_>, method: &str, params: Value) -> R {
        let conn = cx.conn();
        let st = &mut *cx.st;
        let sid = cx.session.clone().unwrap_or_default();
        let me = pid(&sid);
        let now = now_ms();
        st.collab.hub.expire(&mut st.store, now);
        if method != "collab.leave" && method != "collab.join" {
            st.ensure_joined(&sid, None, None)?;
        }
        let result = match method {
            "collab.join" => join(st, conn, &sid, parse(params)?, now),
            "collab.leave" => {
                st.collab.hub.leave(&mut st.store, &me, now);
                st.collab.conns.remove(&me);
                Ok(json!({ "left": true }))
            }
            "collab.presence" => {
                let p: PresenceParams = parse(params)?;
                let presence = Presence {
                    doc: p.doc,
                    camera: p.camera,
                    selection: p.selection.unwrap_or_default(),
                    pointer: p.pointer,
                    activity: p.activity,
                    following: p.following,
                };
                st.collab
                    .hub
                    .set_presence(&mut st.store, &me, presence, now)
                    .map(|()| json!({ "ok": true }))
            }
            "collab.who" => {
                let text = st.collab.hub.who(&st.store, Some(&me), now);
                Ok(json!({
                    "text": text,
                    "participants": st.collab.hub.roster().all().count(),
                    "claims": st.collab.hub.claims().len(),
                }))
            }
            "collab.claim" => claim(st, &me, parse(params)?, now),
            "collab.release" => {
                let p: ReleaseParams = parse(params)?;
                let gone = st
                    .collab
                    .hub
                    .release(&mut st.store, &me, p.claims.as_deref(), now);
                let labels: Vec<&str> = gone.iter().map(|c| c.label.as_str()).collect();
                Ok(json!({
                    "released": gone.len(),
                    "text": format!("released {} claim(s): {}\n", gone.len(), labels.join(", ")),
                }))
            }
            "collab.heartbeat" => {
                st.collab.hub.heartbeat(&mut st.store, &me, now);
                Ok(
                    json!({ "claims": st.collab.hub.claims().iter().filter(|c| c.holder == me).count() }),
                )
            }
            "collab.claims" => Ok(json!({ "claims": st.collab.hub.claims() })),
            "collab.submit" => submit(st, &me, parse(params)?, now),
            "collab.sync" => {
                let p: SyncParams = parse(params)?;
                if p.snapshot.unwrap_or(false) || p.since.is_none() {
                    Ok(
                        json!({ "revision": st.store.revision(), "snapshot": Snapshot::of(&st.store) }),
                    )
                } else {
                    let ops = st.collab.hub.replay(&st.store, p.since.unwrap_or(0));
                    Ok(json!({ "revision": st.store.revision(), "ops": ops }))
                }
            }
            "collab.undo" | "collab.redo" => {
                let redo = method == "collab.redo";
                let out = if redo {
                    st.collab.hub.redo(&mut st.store, &me)
                } else {
                    st.collab.hub.undo(&mut st.store, &me)
                }?;
                let mut v = json!(out);
                if !out.skipped.is_empty() {
                    let fields: Vec<String> = out
                        .skipped
                        .iter()
                        .map(|s| {
                            let path = s.path.as_ref().map(|p| format!(".{p}")).unwrap_or_default();
                            format!("{}{path} ({})", s.record, s.by)
                        })
                        .collect();
                    v["note"] = Diagnostic::info(
                        codes::UNDO_PARTIAL,
                        format!("kept what others changed since: {}", fields.join(", ")),
                    )
                    .message
                    .into();
                }
                Ok(v)
            }
            "collab.conflicts" => {
                let p: ConflictsParams = parse(params)?;
                let list: Vec<_> = if p.mine.unwrap_or(true) {
                    st.collab.hub.conflicts().lost_by(&me).collect()
                } else {
                    st.collab.hub.conflicts().all().collect()
                };
                Ok(json!({ "conflicts": list }))
            }
            "collab.policy" => policy(st, &me, parse(params)?),
            "branch.open" => {
                let p: BranchOpenParams = parse(params)?;
                let info = st.collab.hub.branch_open(&st.store, &me, p.why)?;
                let text = format!(
                    "opened branch {} from revision {}. Apply your turn there with ara_apply {{\"branch\": \"{}\", ...}} (claims do not block you inside it); nothing is live until ara_session {{\"action\": \"submit\", \"branch\": \"{}\"}}, which merges it per the human's apply policy once nobody claims what it touches.\n",
                    info.id, info.base_revision, info.id, info.id
                );
                Ok(with_text(json!(info), text))
            }
            "branch.list" => {
                let list: Vec<_> = st.collab.hub.branches().map(|b| b.info()).collect();
                Ok(json!({ "branches": list }))
            }
            "branch.status" => {
                let p: BranchParams = parse(params)?;
                let info = st.collab.hub.branch(&p.branch)?.info();
                let text = branch_text(&info);
                Ok(with_text(json!(info), text))
            }
            "branch.submit" => {
                let p: BranchSubmitParams = parse(params)?;
                let info =
                    st.collab
                        .hub
                        .branch_submit(&mut st.store, &me, &p.branch, p.why, now)?;
                let text = branch_text(&info);
                Ok(with_text(json!(info), text))
            }
            "branch.merge" => {
                let p: BranchMergeParams = parse(params)?;
                let prefer = match p.prefer {
                    Some(PreferSide::Branch) => Prefer::Branch,
                    _ => Prefer::Live,
                };
                st.collab
                    .hub
                    .branch_merge(
                        &mut st.store,
                        &me,
                        &p.branch,
                        prefer,
                        p.force.unwrap_or(false),
                    )
                    .map(|r| json!(r))
            }
            "branch.discard" => {
                let p: BranchParams = parse(params)?;
                st.collab
                    .hub
                    .branch_discard(&me, &p.branch)
                    .map(|i| json!(i))
            }
            other => Err(vec![Diagnostic::error(
                kernel::UNKNOWN_METHOD,
                format!("collab does not handle `{other}`"),
            )]),
        };
        st.collab_flush();
        result
    }

    fn tick(&mut self, cx: &mut Cx<'_>) {
        let st = &mut *cx.st;
        st.collab.hub.tick(&mut st.store, now_ms());
        st.collab_after();
    }
}

fn with_text(mut v: Value, text: String) -> Value {
    v["text"] = text.into();
    v
}

/// One branch's state as text.
fn branch_text(info: &ara_collab::BranchInfo) -> String {
    let mut out = format!(
        "branch {} by {} ({}), forked at revision {}",
        info.id,
        info.name,
        info.status.name(),
        info.base_revision
    );
    for c in &info.changes {
        let n = |v: &Vec<ara_doc::RecordId>| {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("\n  {}:", c.doc));
        for (verb, list) in [
            ("created", &c.created),
            ("modified", &c.modified),
            ("deleted", &c.deleted),
        ] {
            if !list.is_empty() {
                out.push_str(&format!(" {verb} {};", n(list)));
            }
        }
    }
    if !info.blocked_by.is_empty() {
        out.push_str(&format!(
            "\n  waiting on: {} (it merges by itself once they release it)",
            info.blocked_by.join(", ")
        ));
    }
    if let Some(r) = &info.report {
        out.push_str(&format!(
            "\n  merged at revision {} ({} edits, {} conflicts kept the {} value)",
            r.revision,
            r.edits,
            r.conflicts.len(),
            r.conflicts.first().map_or("live", |c| match c.kept {
                Prefer::Live => "live",
                Prefer::Branch => "branch",
            })
        ));
    }
    out.push('\n');
    out
}

fn join(st: &mut State, conn: ConnId, sid: &str, p: JoinParams, now: u64) -> R {
    let participant = st.ensure_joined(sid, p.name, p.color)?;
    let participant = if participant.is_null() {
        json!(st.collab.hub.roster().get(&pid(sid)))
    } else {
        participant
    };
    st.collab.conns.insert(pid(sid), conn);
    let me = pid(sid);
    let presence: BTreeMap<&ParticipantId, &Presence> = st.collab.hub.presence().all().collect();
    let branches: Vec<_> = st.collab.hub.branches().map(|b| b.info()).collect();
    let conflicts: Vec<_> = st.collab.hub.conflicts().lost_by(&me).collect();
    let participants: Vec<_> = st.collab.hub.roster().all().collect();
    let (policy, idle) = st.collab.hub.policy();
    let mut out = json!({
        "participant": participant,
        "revision": st.store.revision(),
        "participants": participants,
        "presence": presence,
        "claims": st.collab.hub.claims(),
        "conflicts": conflicts,
        "branches": branches,
        "policy": { "policy": policy, "idleMs": idle },
        "text": st.collab.hub.who(&st.store, Some(&me), now),
    });
    if p.snapshot.unwrap_or(false) {
        out["snapshot"] = json!(Snapshot::of(&st.store));
    }
    Ok(out)
}

fn claim(st: &mut State, me: &ParticipantId, p: ClaimParams, now: u64) -> R {
    let mut wanted = Vec::new();
    let mut problems = Vec::new();
    for (i, s) in p.subjects.iter().enumerate() {
        match subject::parse(st, s, p.doc.as_deref()) {
            Ok(w) => wanted.push(w),
            Err(d) => problems.push(d.path(format!("subjects[{i}]"))),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let ttl = p.ttl_ms.unwrap_or(DEFAULT_TTL_MS);
    let granted = st
        .collab
        .hub
        .claim(&mut st.store, me, wanted, ttl, p.why, now)?;
    let labels: Vec<&str> = granted.claims.iter().map(|c| c.label.as_str()).collect();
    let mut text = format!(
        "claimed {} for {} s (any collab call or presence update extends it)",
        labels.join(", "),
        ttl / 1000
    );
    for o in &granted.overlaps {
        text.push_str(&format!("\n  note: {} also claims {}", o.name, o.label));
    }
    text.push('\n');
    Ok(json!({ "claims": granted.claims, "overlaps": granted.overlaps, "text": text }))
}

fn submit(st: &mut State, me: &ParticipantId, p: SubmitParams, now: u64) -> R {
    let edits: Vec<Edit> = p
        .edits
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            serde_json::from_value(e).map_err(|err| {
                Diagnostic::error(kernel::INVALID_PARAMS, format!("edit {i}: {err}"))
                    .path(format!("edits[{i}]"))
            })
        })
        .collect::<Result<_, _>>()
        .map_err(|d| vec![d])?;
    let s = Submit {
        seq: p.seq,
        base: p.base,
        edits,
        why: p.why,
        label: p.label,
    };
    st.collab
        .hub
        .submit(&mut st.store, me, s, now)
        .map(|ack| json!(ack))
}

fn policy(st: &mut State, me: &ParticipantId, p: PolicyParams) -> R {
    if p.policy.is_some() || p.idle_ms.is_some() {
        let human = st
            .collab
            .hub
            .roster()
            .get(me)
            .is_some_and(|x| x.kind == Kind::Human);
        if !human {
            return Err(vec![Diagnostic::error(
                codes::HUMAN_ONLY,
                "only human sessions set when agent branches merge (D20)",
            )]);
        }
        let current = st.collab.hub.policy().0;
        let policy = match p.policy {
            Some(PolicyName::Immediately) => ApplyPolicy::Immediately,
            Some(PolicyName::WhenIdle) => ApplyPolicy::WhenIdle,
            Some(PolicyName::OnApproval) => ApplyPolicy::OnApproval,
            None => current,
        };
        st.collab.hub.set_policy(policy, p.idle_ms);
    }
    let (policy, idle) = st.collab.hub.policy();
    Ok(json!({ "policy": policy, "idleMs": idle }))
}

/// `cmd.apply` with `branch`: runs the batch in the caller's branch session. No
/// checkpoint, no review entry, nothing live; the branch is persisted.
pub(crate) fn branch_apply(st: &mut State, sid: &str, branch: &str, p: &ApplyParams) -> R {
    let session = st
        .sessions
        .live(sid)
        .cloned()
        .ok_or_else(|| vec![session_not_found(sid)])?;
    st.ensure_joined(sid, None, None)?;
    let (doc, batch) = st.batch(p)?;
    let registry = &st.registry;
    let store = st.collab.hub.branch_store(branch, &pid(sid))?;
    let result = registry
        .apply(store, &batch, &session.cmd_session())
        .map_err(relocate)?;
    let mut text = apply_text(st, &doc, &result, None);
    let verb = if result.dry_run {
        "would apply"
    } else {
        "applied"
    };
    text = format!(
        "{verb} in branch {branch}, not live (submit with ara_session {{\"action\": \"submit\", \"branch\": \"{branch}\"}}): {text}"
    );
    st.collab_flush();
    Ok(json!({
        "ok": true,
        "branch": branch,
        "doc": crate::core::path_of(st, &doc),
        "text": text,
        "dryRun": result.dry_run,
        "txn": result.txn,
        "revision": result.revision,
        "ids": result.symbols,
        "diff": diff_summary(&result.ops),
        "ops": result.ops,
    }))
}
