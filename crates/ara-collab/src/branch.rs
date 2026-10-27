//! Branch sessions (D20): an agent works on an isolated copy of the live documents
//! (an overlay transaction stack) and submits the turn; the human's live view changes only when the optional branch is submitted.
//!
//! A merge is a field-level three-way merge per touched document (`ara_merge`):
//! base = the documents when the branch was forked, ours = live now, theirs = the
//! branch. Fields only the branch changed apply; fields both changed keep the live
//! value (or the branch's, with `prefer: branch`) and are reported. The result
//! lands as one ordinary transaction by the branch's author, so it is reviewable and
//! undoable like any agent turn. Submitted branches merge immediately.

use std::collections::BTreeSet;

use ara_diag::{Diagnostic, codes as kernel};
use ara_doc::{AssetId, Document, Edit, IdGen, RecordId, read, write};
use ara_merge::{diff, merge};
use ara_ops::{Author, Store, Transaction, TxnId, TxnMeta};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::codes;
use crate::hub::{Audience, Hub, Origin};
use crate::participant::{Kind, ParticipantId};
use crate::sync::{Conflict, ConflictKind, Side, methods};

/// Where a branch session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchStatus {
    /// The owner is working in it.
    Open,
    /// Submitted; merges immediately.
    Submitted,
    /// Merged into the live documents.
    Merged,
    /// Thrown away.
    Discarded,
}

/// Which side wins a field both the live documents and the branch changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Prefer {
    /// Keep the live value (the default: the human's work is never overwritten).
    #[default]
    Live,
    /// Take the branch's value.
    Branch,
}

/// A field both sides changed differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflict {
    /// Document.
    pub doc: AssetId,
    /// Record ID.
    pub record: String,
    /// Field path (empty: the whole record).
    pub path: String,
    /// The value when the branch was forked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Value>,
    /// The live value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<Value>,
    /// The branch's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Value>,
    /// The side kept.
    pub kept: Prefer,
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    /// The merge transaction (absent if the branch changed nothing live lacks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txn: Option<TxnId>,
    /// Live revision afterwards.
    pub revision: u64,
    /// Edits applied.
    pub edits: usize,
    /// Fields both sides changed.
    pub conflicts: Vec<MergeConflict>,
    /// Who explicitly merged it (`None`: the owner submitted it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<ParticipantId>,
}

/// What a branch changed in one document, relative to its fork.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocChanges {
    /// Document path.
    pub doc: String,
    /// Created records.
    pub created: Vec<RecordId>,
    /// Modified records.
    pub modified: Vec<RecordId>,
    /// Deleted records.
    pub deleted: Vec<RecordId>,
}

/// A branch's summary: what `branch.*` methods return and `collab.branch` carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    /// Branch ID (`br_…`).
    pub id: String,
    /// Owner (session).
    pub owner: ParticipantId,
    /// Owner's display name.
    pub name: String,
    /// The turn's why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Live revision it was forked from.
    pub base_revision: u64,
    /// Transactions in the branch.
    pub transactions: usize,
    /// Status.
    pub status: BranchStatus,
    /// Changes per document.
    pub changes: Vec<DocChanges>,
    /// Validation errors that prevented an optional branch merge.
    pub blocked_by: Vec<String>,
    /// The merge result, once merged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<MergeReport>,
}

/// One branch session.
#[derive(Debug)]
pub struct Branch {
    /// Branch ID (`br_…`).
    pub id: String,
    /// Owner (session).
    pub owner: ParticipantId,
    /// Author of its transactions and of the merge.
    pub author: Author,
    /// Owner's display name.
    pub name: String,
    /// The turn's why.
    pub why: Option<String>,
    /// Live revision it was forked from.
    pub base_revision: u64,
    /// Status.
    pub status: BranchStatus,
    /// The merge result, once merged.
    pub report: Option<MergeReport>,
    /// Validation errors from the latest merge attempt.
    pub blocked_by: Vec<String>,
    store: Store,
}

/// A document's canonical text, for persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDoc {
    /// Project path.
    pub path: String,
    /// Canonical text.
    pub text: String,
}

/// A branch as persisted in `.ara/branches/<id>.json`: its touched documents at the
/// fork plus its transactions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedBranch {
    /// Branch ID.
    pub id: String,
    /// Owner (session).
    pub owner: ParticipantId,
    /// Author.
    pub author: Author,
    /// Owner's display name.
    pub name: String,
    /// The turn's why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Live revision at the fork.
    pub base_revision: u64,
    /// Status.
    pub status: BranchStatus,
    /// The merge result, once merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<MergeReport>,
    /// Touched documents as they were at the fork.
    pub base: Vec<SavedDoc>,
    /// The branch's transactions, oldest first.
    pub txns: Vec<Transaction>,
}

fn seed(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Branch {
    /// Forks the live documents for `owner`.
    pub fn fork(
        id: String,
        owner: ParticipantId,
        author: Author,
        name: String,
        why: Option<String>,
        live: &Store,
    ) -> Branch {
        let store = Store::with_ids(
            live.schema().clone(),
            live.workspace().clone(),
            IdGen::seeded(seed(&id)),
        );
        Branch {
            id,
            owner,
            author,
            name,
            why,
            base_revision: live.revision(),
            status: BranchStatus::Open,
            report: None,
            blocked_by: Vec::new(),
            store,
        }
    }

    /// The overlay: documents with the branch's edits, and its history.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The overlay, for running commands in the branch (open branches only; the
    /// host checks).
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Documents the branch's transactions touched.
    pub fn touched(&self) -> BTreeSet<AssetId> {
        self.store
            .history()
            .iter()
            .flat_map(|t| t.ops.iter().map(|op| op.doc().clone()))
            .collect()
    }

    /// `doc` as it was when the branch was forked (its history reverted).
    pub fn base_doc(&self, doc: &AssetId) -> Option<Document> {
        let mut workspace = self.store.workspace().clone();
        for t in self.store.history().iter().rev() {
            for op in t.ops.iter().rev() {
                workspace.revert(op, self.store.schema());
            }
        }
        workspace.doc(doc.as_str()).cloned()
    }

    fn path(&self, doc: &AssetId) -> String {
        self.store
            .workspace()
            .path_of(doc.as_str())
            .map(str::to_owned)
            .or_else(|| {
                self.store
                    .history()
                    .iter()
                    .flat_map(|txn| &txn.ops)
                    .find_map(|op| match op {
                        ara_doc::Op::Document { doc: id, path, .. } if id == doc => {
                            Some(path.clone())
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| doc.to_string())
    }

    /// What the branch changed, per touched document.
    pub fn changes(&self) -> Vec<DocChanges> {
        let ws = self.store.workspace();
        self.touched()
            .into_iter()
            .map(|doc| {
                let base = self.base_doc(&doc);
                let now = ws.doc(doc.as_str());
                let mut c = DocChanges {
                    doc: self.path(&doc),
                    created: Vec::new(),
                    modified: Vec::new(),
                    deleted: Vec::new(),
                };
                let edits = match (base.as_ref(), now) {
                    (Some(base), Some(now)) => diff(base, now),
                    (None, Some(now)) => {
                        c.created.extend(now.records().keys().cloned());
                        Vec::new()
                    }
                    (Some(base), None) => {
                        c.deleted.extend(base.records().keys().cloned());
                        Vec::new()
                    }
                    (None, None) => Vec::new(),
                };
                for edit in edits {
                    match edit {
                        // `diff` compares records of two existing documents.
                        Edit::Document { .. } | Edit::File { .. } => {}
                        Edit::Create { id, .. } => c.created.push(id),
                        Edit::Delete { id, .. } => c.deleted.push(id),
                        Edit::Set { id, .. } | Edit::Move { id, .. } => {
                            if !c.modified.contains(&id) {
                                c.modified.push(id);
                            }
                        }
                    }
                }
                c
            })
            .collect()
    }

    /// The summary.
    pub fn info(&self) -> BranchInfo {
        BranchInfo {
            id: self.id.clone(),
            owner: self.owner.clone(),
            name: self.name.clone(),
            why: self.why.clone(),
            base_revision: self.base_revision,
            transactions: self.store.history().len(),
            status: self.status,
            changes: self.changes(),
            blocked_by: self.blocked_by.clone(),
            report: self.report.clone(),
        }
    }

    /// Merges into `live` (see the module docs). Commits one transaction by the
    /// branch's author with `origin` cleared of the guard; the caller checks claims.
    fn merge_into(
        &self,
        live: &mut Store,
        prefer: Prefer,
    ) -> Result<(Option<Transaction>, usize, Vec<MergeConflict>), Vec<Diagnostic>> {
        let mut edits = Vec::new();
        let mut conflicts = Vec::new();
        for doc in self.touched() {
            let base = self.base_doc(&doc);
            let ours = live.workspace().doc(doc.as_str()).cloned();
            let theirs = self.store.workspace().doc(doc.as_str());
            if base.is_none() && ours.is_none() {
                if let Some(theirs) = theirs {
                    let path = self
                        .store
                        .workspace()
                        .path_of(doc.as_str())
                        .expect("loaded document has a path");
                    edits.push(Edit::Document {
                        doc: doc.clone(),
                        path: path.into(),
                        kind: Some(theirs.kind.clone()),
                    });
                    edits.extend(theirs.records().iter().map(|(id, record)| Edit::Create {
                        doc: doc.clone(),
                        id: id.clone(),
                        record: record.clone(),
                    }));
                }
                continue;
            }
            if base.is_none() {
                if ours.as_ref() == theirs {
                    continue;
                }
                return Err(vec![Diagnostic::error(
                    kernel::STALE_BASE,
                    "a document with this identity was created concurrently",
                )]);
            }
            if theirs.is_none() && ours.as_ref() == base.as_ref() {
                if let Some(ours) = &ours {
                    edits.extend(ours.records().keys().map(|id| Edit::Delete {
                        doc: doc.clone(),
                        id: id.clone(),
                    }));
                    let path = live
                        .workspace()
                        .path_of(doc.as_str())
                        .expect("loaded document has a path");
                    edits.push(Edit::Document {
                        doc: doc.clone(),
                        path: path.into(),
                        kind: None,
                    });
                }
                continue;
            }
            let (Some(base), Some(ours), Some(theirs)) = (base, ours, theirs) else {
                return Err(vec![Diagnostic::error(
                    kernel::DOC_NOT_FOUND,
                    format!("`{doc}` is no longer loaded; the branch cannot merge"),
                )]);
            };
            let merged = match prefer {
                Prefer::Live => merge(&base, &ours, theirs, live.schema()),
                Prefer::Branch => merge(&base, theirs, &ours, live.schema()),
            }
            .map_err(|d| vec![d])?;
            for c in &merged.conflicts {
                let (live_v, branch_v) = match prefer {
                    Prefer::Live => (c.ours.clone(), c.theirs.clone()),
                    Prefer::Branch => (c.theirs.clone(), c.ours.clone()),
                };
                conflicts.push(MergeConflict {
                    doc: doc.clone(),
                    record: c.record.clone(),
                    path: c.path.clone(),
                    base: c.base.clone(),
                    live: live_v,
                    branch: branch_v,
                    kept: prefer,
                });
            }
            edits.extend(diff(&ours, &merged.doc));
        }
        let n = edits.len();
        if edits.is_empty() {
            return Ok((None, 0, conflicts));
        }
        let meta = TxnMeta {
            author: self.author.clone(),
            why: Some(
                self.why
                    .clone()
                    .unwrap_or_else(|| format!("merge branch {}", self.id)),
            ),
            label: Some(format!("branch.merge {}", self.id)),
            base: None,
            idempotency_key: Some(format!("merge:{}", self.id)),
        };
        let guard = live.set_guard(None);
        let result = live.transact(meta, edits).map(|c| c.txn().cloned());
        live.set_guard(guard);
        Ok((result?, n, conflicts))
    }

    /// The persisted form.
    pub fn save(&self) -> SavedBranch {
        let base = self
            .touched()
            .into_iter()
            .filter_map(|doc| {
                let d = self.base_doc(&doc)?;
                Some(SavedDoc {
                    path: self.path(&doc),
                    text: write(&d, self.store.schema()),
                })
            })
            .collect();
        SavedBranch {
            id: self.id.clone(),
            owner: self.owner.clone(),
            author: self.author.clone(),
            name: self.name.clone(),
            why: self.why.clone(),
            base_revision: self.base_revision,
            status: self.status,
            report: self.report.clone(),
            base,
            txns: self.store.history().to_vec(),
        }
    }

    /// Rebuilds a persisted branch over the current live documents: its touched
    /// documents go back to their fork state, then its transactions are replayed.
    pub fn load(saved: SavedBranch, live: &Store) -> Result<Branch, Vec<Diagnostic>> {
        let mut ws = live.workspace().clone();
        for txn in &saved.txns {
            for op in &txn.ops {
                if let ara_doc::Op::Document {
                    doc,
                    kind: Some(_),
                    prev: None,
                    ..
                } = op
                {
                    ws.remove(doc.as_str());
                }
            }
        }
        for d in &saved.base {
            let read = read(&d.text, &d.path, live.schema(), Some(ws.index()));
            let doc = read.doc.ok_or(read.diags)?;
            ws.insert(d.path.clone(), doc);
        }
        let mut store = Store::with_ids(live.schema().clone(), ws, IdGen::seeded(seed(&saved.id)));
        for t in saved.txns {
            store.restore(t, true)?;
        }
        Ok(Branch {
            id: saved.id,
            owner: saved.owner,
            author: saved.author,
            name: saved.name,
            why: saved.why,
            base_revision: saved.base_revision,
            status: saved.status,
            report: saved.report,
            blocked_by: Vec::new(),
            store,
        })
    }
}

fn not_found(id: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        codes::BRANCH_NOT_FOUND,
        format!("no branch session `{id}`; list them with `branch.list`"),
    )]
}

fn wrong_state(b: &Branch, want: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        codes::BRANCH_STATE,
        format!(
            "branch `{}` is {}; it must be {want}",
            b.id,
            b.status.name()
        ),
    )]
}

impl Hub {
    /// Forks the live documents into a new branch session owned by `id`.
    pub fn branch_open(
        &mut self,
        live: &Store,
        id: &ParticipantId,
        why: Option<String>,
    ) -> Result<BranchInfo, Vec<Diagnostic>> {
        let p = self.participant(id)?.clone();
        let bid = self.ids.token("br", 8);
        let b = Branch::fork(bid.clone(), id.clone(), p.author, p.name, why, live);
        let info = b.info();
        self.branches.insert(bid.clone(), b);
        self.branch_changed(&bid);
        Ok(info)
    }

    /// A branch.
    pub fn branch(&self, branch: &str) -> Result<&Branch, Vec<Diagnostic>> {
        self.branches.get(branch).ok_or_else(|| not_found(branch))
    }

    /// Every branch session, by ID.
    pub fn branches(&self) -> impl Iterator<Item = &Branch> {
        self.branches.values()
    }

    /// True if `id` may edit or submit `b`: its owner session, or another session of
    /// the same agent (CLI calls and reconnects begin new sessions).
    fn owns(&self, b: &Branch, id: &ParticipantId) -> bool {
        &b.owner == id
            || (matches!(b.author, Author::Agent(_))
                && self.roster.get(id).is_some_and(|p| p.author == b.author))
    }

    /// The overlay store of `id`'s open branch, to run commands in. Marks the branch
    /// changed (persist it afterwards).
    pub fn branch_store(
        &mut self,
        branch: &str,
        id: &ParticipantId,
    ) -> Result<&mut Store, Vec<Diagnostic>> {
        let b = self.branches.get(branch).ok_or_else(|| not_found(branch))?;
        if !self.owns(b, id) {
            return Err(vec![Diagnostic::error(
                codes::BRANCH_STATE,
                format!(
                    "branch `{branch}` belongs to {}; only its owner edits it",
                    b.name
                ),
            )]);
        }
        if b.status != BranchStatus::Open {
            return Err(wrong_state(b, "open"));
        }
        self.dirty.insert(branch.to_owned());
        let b = self
            .branches
            .get_mut(branch)
            .ok_or_else(|| not_found(branch))?;
        Ok(b.store_mut())
    }

    /// Submit an optional branch and merge it immediately.
    pub fn branch_submit(
        &mut self,
        live: &mut Store,
        id: &ParticipantId,
        branch: &str,
        why: Option<String>,
        now: u64,
    ) -> Result<BranchInfo, Vec<Diagnostic>> {
        let b = self.branches.get(branch).ok_or_else(|| not_found(branch))?;
        if !self.owns(b, id) {
            return Err(vec![Diagnostic::error(
                codes::BRANCH_STATE,
                format!("branch `{branch}` belongs to {}", b.name),
            )]);
        }
        let b = self
            .branches
            .get_mut(branch)
            .ok_or_else(|| not_found(branch))?;
        if b.status != BranchStatus::Open {
            return Err(wrong_state(b, "open"));
        }
        b.status = BranchStatus::Submitted;
        if why.is_some() {
            b.why = why;
        }
        self.branch_changed(branch);
        self.merge_due(live, now);
        self.branch(branch).map(Branch::info)
    }

    /// Discards a branch (its owner or a human).
    pub fn branch_discard(
        &mut self,
        id: &ParticipantId,
        branch: &str,
    ) -> Result<BranchInfo, Vec<Diagnostic>> {
        let human = self.participant(id)?.kind == Kind::Human;
        let owns = self.branch(branch).is_ok_and(|b| self.owns(b, id));
        let b = self
            .branches
            .get_mut(branch)
            .ok_or_else(|| not_found(branch))?;
        if !owns && !human {
            return Err(vec![Diagnostic::error(
                codes::HUMAN_ONLY,
                format!("only {} or a human can discard `{branch}`", b.name),
            )]);
        }
        if matches!(b.status, BranchStatus::Merged | BranchStatus::Discarded) {
            return Err(wrong_state(b, "open or submitted"));
        }
        b.status = BranchStatus::Discarded;
        self.branch_changed(branch);
        self.branch(branch).map(Branch::info)
    }

    /// Explicitly merge an optional branch (human session); `force` is a legacy no-op.
    pub fn branch_merge(
        &mut self,
        live: &mut Store,
        approver: &ParticipantId,
        branch: &str,
        prefer: Prefer,
        _force: bool,
    ) -> Result<MergeReport, Vec<Diagnostic>> {
        if self.participant(approver)?.kind != Kind::Human {
            return Err(vec![Diagnostic::error(
                codes::HUMAN_ONLY,
                "only human sessions approve branch merges; agents submit (`branch.submit`)",
            )]);
        }
        let b = self.branch(branch)?;
        if matches!(b.status, BranchStatus::Merged | BranchStatus::Discarded) {
            return Err(wrong_state(b, "open or submitted"));
        }
        self.merge_now(live, branch, prefer, Some(approver.clone()))
    }

    /// Merges submitted optional branches immediately.
    pub(crate) fn merge_due(&mut self, live: &mut Store, now: u64) {
        let submitted: Vec<String> = self
            .branches
            .values()
            .filter(|b| b.status == BranchStatus::Submitted)
            .filter(|b| {
                self.gesture_branches
                    .get(&b.id)
                    .is_none_or(|deadline| now >= *deadline)
            })
            .map(|b| b.id.clone())
            .collect();
        for id in submitted {
            if let Err(diags) = self.merge_now(live, &id, Prefer::Live, None) {
                if let Some(b) = self.branches.get_mut(&id) {
                    b.blocked_by = diags.iter().map(|d| d.message.clone()).collect();
                }
                self.branch_changed(&id);
            }
        }
    }

    fn merge_now(
        &mut self,
        live: &mut Store,
        branch: &str,
        prefer: Prefer,
        approver: Option<ParticipantId>,
    ) -> Result<MergeReport, Vec<Diagnostic>> {
        let b = self.branches.get(branch).ok_or_else(|| not_found(branch))?;
        let (txn, edits, conflicts) = b.merge_into(live, prefer)?;
        let (owner, owner_name, base_revision) = (b.owner.clone(), b.name.clone(), b.base_revision);
        if let Some(t) = &txn {
            self.origins.insert(
                t.id.clone(),
                Origin {
                    participant: Some(owner.clone()),
                    seq: None,
                    base: base_revision,
                },
            );
        }
        for c in &conflicts {
            let (lost, won, loser_name) = match c.kept {
                Prefer::Live => (c.branch.clone(), c.live.clone(), owner_name.clone()),
                Prefer::Branch => (c.live.clone(), c.branch.clone(), "live".to_owned()),
            };
            let entry = Conflict {
                id: 0,
                kind: ConflictKind::Merge,
                revision: live.revision(),
                doc: c.doc.clone(),
                record: RecordId::parse(&c.record).ok(),
                path: c.path.parse().ok(),
                loser: Side {
                    participant: (c.kept == Prefer::Live).then(|| owner.clone()),
                    name: loser_name,
                    txn: None,
                    seq: None,
                    value: lost,
                },
                winner: Some(Side {
                    participant: None,
                    name: if c.kept == Prefer::Live {
                        "live".into()
                    } else {
                        owner_name.clone()
                    },
                    txn: txn.as_ref().map(|t| t.id.clone()),
                    seq: None,
                    value: won,
                }),
                message: format!(
                    "branch `{branch}` and the live scene both changed `{}{}{}`; the {} value was kept",
                    c.record,
                    if c.path.is_empty() { "" } else { "." },
                    c.path,
                    if c.kept == Prefer::Live {
                        "live"
                    } else {
                        "branch"
                    }
                ),
            };
            let entry = self.conflicts.push(entry);
            if let Some(p) = entry.loser.participant.clone() {
                let params = serde_json::to_value(&entry).unwrap_or(Value::Null);
                self.emit(Audience::Only(p), methods::CONFLICT, params);
            }
        }
        let report = MergeReport {
            txn: txn.map(|t| t.id),
            revision: live.revision(),
            edits,
            conflicts,
            approved_by: approver,
        };
        if let Some(b) = self.branches.get_mut(branch) {
            b.status = BranchStatus::Merged;
            b.report = Some(report.clone());
            b.blocked_by.clear();
        }
        self.branch_changed(branch);
        Ok(report)
    }

    fn branch_changed(&mut self, branch: &str) {
        self.dirty.insert(branch.to_owned());
        if let Some(b) = self.branches.get(branch) {
            let params = json!({ "branch": b.info() });
            self.emit(Audience::All, methods::BRANCH, params);
        }
    }

    /// Branches changed since the last call, in persisted form (the host writes them
    /// to `.ara/branches/`).
    pub fn take_dirty_branches(&mut self) -> Vec<SavedBranch> {
        let dirty = std::mem::take(&mut self.dirty);
        dirty
            .iter()
            .filter_map(|id| self.branches.get(id).map(Branch::save))
            .collect()
    }

    /// Restores a persisted branch.
    pub fn restore_branch(
        &mut self,
        saved: SavedBranch,
        live: &Store,
    ) -> Result<(), Vec<Diagnostic>> {
        let b = Branch::load(saved, live)?;
        self.branches.insert(b.id.clone(), b);
        Ok(())
    }
}

impl BranchStatus {
    /// The wire name (`submitted`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Submitted => "submitted",
            Self::Merged => "merged",
            Self::Discarded => "discarded",
        }
    }
}
