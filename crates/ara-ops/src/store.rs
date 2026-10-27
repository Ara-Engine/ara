//! [`Store`]: the authority over a workspace's documents. Every change goes through a
//! [`Staged`] transaction and lands in the history.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, Sender, channel};

use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document, Edit, IdGen, Op, RecordId, Workspace};
use ara_schema::{FieldPath, SchemaDb};

use crate::{Author, Guard, Transaction, TxnId, TxnMeta};

/// The result of committing.
#[derive(Debug)]
pub enum Commit<'a> {
    /// Applied as a new transaction.
    Applied(&'a Transaction),
    /// The idempotency key was already used; nothing changed and this is the original.
    Replayed(&'a Transaction),
    /// There were no ops; nothing was recorded.
    Empty,
}

impl<'a> Commit<'a> {
    /// The recorded transaction, if any.
    pub fn txn(&self) -> Option<&'a Transaction> {
        match self {
            Commit::Applied(t) | Commit::Replayed(t) => Some(t),
            Commit::Empty => None,
        }
    }
}

/// The result of [`Store::undo_rebased`].
#[derive(Debug)]
pub struct Rebased<'a> {
    /// The undo transaction; `None` when every op was skipped.
    pub txn: Option<&'a Transaction>,
    /// The target's ops left in place because someone else changed the same paths
    /// later, in their original order.
    pub skipped: Vec<Op>,
}

#[derive(Debug, Default, Clone)]
struct Stacks {
    undo: Vec<TxnId>,
    redo: Vec<TxnId>,
}

/// Documents plus their attributed, undoable history.
///
/// Open a transaction with [`begin`](Self::begin), apply edits (each takes effect
/// immediately, so later reads see it), then [`commit`](Staged::commit). Commit
/// validates every touched document and either records everything or nothing.
#[derive(Debug)]
pub struct Store {
    db: SchemaDb,
    ws: Workspace,
    rev: u64,
    history: Vec<Transaction>,
    by_id: HashMap<TxnId, usize>,
    by_key: HashMap<String, usize>,
    reverted_by: HashMap<TxnId, TxnId>,
    stacks: BTreeMap<Author, Stacks>,
    ids: IdGen,
    subscribers: Vec<Sender<Transaction>>,
    guard: Option<Box<dyn Guard>>,
    origin: Option<String>,
}

impl Store {
    /// A store over `ws`, at revision 0. IDs are minted from entropy.
    pub fn new(db: SchemaDb, ws: Workspace) -> Self {
        Self::with_ids(db, ws, IdGen::from_entropy())
    }

    /// A store with a given ID generator (seed it for reproducible IDs).
    pub fn with_ids(db: SchemaDb, ws: Workspace, ids: IdGen) -> Self {
        Store {
            db,
            ws,
            rev: 0,
            history: Vec::new(),
            by_id: HashMap::new(),
            by_key: HashMap::new(),
            reverted_by: HashMap::new(),
            stacks: BTreeMap::new(),
            ids,
            subscribers: Vec::new(),
            guard: None,
            origin: None,
        }
    }

    /// The Schema DB.
    pub fn schema(&self) -> &SchemaDb {
        &self.db
    }

    /// The Schema DB, to register runtime types. Call `verify()` afterwards.
    pub fn schema_mut(&mut self) -> &mut SchemaDb {
        &mut self.db
    }

    /// The documents.
    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// Loads a document that appeared on disk (or replaces one by ID). This is not an
    /// edit: no op is recorded. Record changes always go through transactions.
    pub fn load_doc(&mut self, path: impl Into<String>, doc: Document) {
        self.ws.insert(path, doc);
    }

    /// Loads a sidecar at `path` while its asset ID resolves to `asset_path`.
    /// Like `load_doc`, this records no ops; record changes require transactions.
    pub fn load_doc_with_asset_path(
        &mut self,
        path: impl Into<String>,
        doc: Document,
        asset_path: impl Into<String>,
    ) {
        self.ws.insert_with_asset_path(path, doc, asset_path);
    }

    /// Unloads a document whose file was deleted. History that touched it stays, but
    /// undoing it fails with `ARA1506`.
    pub fn unload_doc(&mut self, id: &str) -> Option<(String, Document)> {
        self.ws.remove(id)
    }

    /// The current revision: the number of committed transactions.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// The ID generator, for minting record IDs.
    pub fn ids(&mut self) -> &mut IdGen {
        &mut self.ids
    }

    /// Every committed transaction, oldest first.
    pub fn history(&self) -> &[Transaction] {
        &self.history
    }

    /// Transactions committed after revision `rev`: "what changed since X".
    pub fn since(&self, rev: u64) -> &[Transaction] {
        let start = self.history.partition_point(|t| t.result <= rev);
        &self.history[start..]
    }

    /// A transaction by ID.
    pub fn txn(&self, id: &TxnId) -> Option<&Transaction> {
        self.by_id.get(id).map(|&i| &self.history[i])
    }

    /// The transaction `author`'s next [`undo_last`](Self::undo_last) would revert.
    /// Menus use it to label "Undo ‹command›" without changing the stacks.
    pub fn next_undo(&self, author: &Author) -> Option<&Transaction> {
        self.stacks
            .get(author)
            .and_then(|s| s.undo.last())
            .and_then(|id| self.txn(id))
    }

    /// The undo transaction `author`'s next [`redo_last`](Self::redo_last) would revert;
    /// its `reverts` names the original transaction.
    pub fn next_redo(&self, author: &Author) -> Option<&Transaction> {
        self.stacks
            .get(author)
            .and_then(|s| s.redo.last())
            .and_then(|id| self.txn(id))
    }

    /// Whether a retained transaction is still on its author's undo stack.
    /// Entity-scoped tools use this to select active turns, including redos.
    pub fn undoable(&self, id: &TxnId) -> bool {
        self.txn(id)
            .and_then(|t| self.stacks.get(&t.author))
            .is_some_and(|s| s.undo.contains(id))
    }

    /// The transaction that reverted `id`, if any.
    pub fn reverted_by(&self, id: &TxnId) -> Option<&TxnId> {
        self.reverted_by.get(id)
    }

    /// The transaction committed with this idempotency key, if any.
    pub fn find_by_key(&self, key: &str) -> Option<&Transaction> {
        self.by_key.get(key).map(|&i| &self.history[i])
    }

    /// Subscribes to the op stream: every transaction committed from now on is sent,
    /// in order. Dropping the receiver unsubscribes.
    pub fn subscribe(&mut self) -> Receiver<Transaction> {
        let (tx, rx) = channel();
        self.subscribers.push(tx);
        rx
    }

    /// Installs (or with `None`, removes) the commit guard; returns the previous one.
    /// Every commit and dry-run check asks it after the base-revision check.
    pub fn set_guard(&mut self, guard: Option<Box<dyn Guard>>) -> Option<Box<dyn Guard>> {
        std::mem::replace(&mut self.guard, guard)
    }

    /// Sets who is committing from now on (a session handle), for the guard. The host
    /// sets it around each call; `None` when unknown.
    pub fn set_origin(&mut self, origin: Option<String>) {
        self.origin = origin;
    }

    /// Who is committing, as last set by [`set_origin`](Self::set_origin).
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// Opens a transaction.
    pub fn begin(&mut self, meta: TxnMeta) -> Staged<'_> {
        Staged {
            ids_before: self.ids.clone(),
            store: Some(self),
            meta,
            reverts: None,
            ops: Vec::new(),
            baselines: BTreeMap::new(),
            scope: Scope::Local(None),
        }
    }

    /// Applies `edits` as one transaction.
    pub fn transact(
        &mut self,
        meta: TxnMeta,
        edits: impl IntoIterator<Item = Edit>,
    ) -> Result<Commit<'_>, Vec<Diagnostic>> {
        let mut staged = self.begin(meta);
        for edit in edits {
            staged.apply(edit).map_err(|d| vec![d])?;
        }
        staged.commit()
    }

    /// Puts a journaled transaction back into history after a restart, keeping its ID,
    /// author and revisions. With `apply`, its ops are applied first (the documents on
    /// disk predate it); without, the documents already contain it. Subscribers are not
    /// notified. Undo and redo stacks use the same transitions as live commits.
    ///
    /// Fails with `ARA1501` unless `txn.result` is past the current revision, and with
    /// the op's diagnostic if an op no longer applies (nothing changes then).
    pub fn restore(&mut self, txn: Transaction, apply: bool) -> Result<(), Vec<Diagnostic>> {
        self.restore_selected(txn, |_| apply)
    }

    /// Restores full transaction history while replaying only documents whose files
    /// predate it. Recovery uses this when a crash interrupted a multi-document flush.
    pub fn restore_documents(
        &mut self,
        txn: Transaction,
        documents: &BTreeSet<AssetId>,
    ) -> Result<(), Vec<Diagnostic>> {
        self.restore_selected(txn, |op| documents.contains(op.doc()))
    }

    fn restore_selected(
        &mut self,
        txn: Transaction,
        apply: impl Fn(&Op) -> bool,
    ) -> Result<(), Vec<Diagnostic>> {
        if txn.result <= self.rev {
            return Err(vec![Diagnostic::error(
                codes::STALE_BASE,
                format!(
                    "`{}` ends at revision {}, but the store is already at {}",
                    txn.id, txn.result, self.rev
                ),
            )]);
        }
        {
            let mut applied = Vec::new();
            for op in txn.ops.iter().filter(|op| apply(op)) {
                let result = self.ws.apply(op.to_edit(), &self.db);
                match result {
                    Ok(done) => applied.push(done),
                    Err(d) => {
                        self.rollback(&applied);
                        return Err(vec![d]);
                    }
                }
            }
        }
        self.rev = txn.result;
        let index = self.history.len();
        if let Some(key) = &txn.idempotency_key {
            self.by_key.insert(key.clone(), index);
        }
        self.record_stacks(&txn);
        self.by_id.insert(txn.id.clone(), index);
        self.history.push(txn);
        Ok(())
    }

    /// Undoes one transaction by applying its inverse as a new transaction by `by`.
    /// Fails with `ARA1502` if a later transaction touched the same paths (unless
    /// that transaction was itself undone).
    pub fn undo(&mut self, id: &TxnId, by: Author) -> Result<&Transaction, Vec<Diagnostic>> {
        let undone_author = self.txn(id).map(|t| t.author.clone());
        let new = self.revert(id, by)?;
        if let Some(stacks) = undone_author.and_then(|a| self.stacks.get_mut(&a)) {
            stacks.undo.retain(|t| t != id);
        }
        Ok(&self.history[new])
    }

    /// Undoes `author`'s most recent transaction that is still in effect.
    pub fn undo_last(&mut self, author: &Author) -> Result<&Transaction, Vec<Diagnostic>> {
        let Some(id) = self.stacks.get(author).and_then(|s| s.undo.last()).cloned() else {
            return Err(vec![nothing(author, "undo")]);
        };
        let new = self.revert(&id, author.clone())?;
        Ok(&self.history[new])
    }

    /// Redoes `author`'s most recently undone transaction. A new edit by the author
    /// clears the redo stack.
    pub fn redo_last(&mut self, author: &Author) -> Result<&Transaction, Vec<Diagnostic>> {
        let Some(id) = self.stacks.get(author).and_then(|s| s.redo.last()).cloned() else {
            return Err(vec![nothing(author, "redo")]);
        };
        let new = self.revert(&id, author.clone())?;
        Ok(&self.history[new])
    }

    /// Per-participant undo (D21): undoes `id` without undoing anyone else's work.
    ///
    /// Each op's inverse is applied, newest first, unless a later still-effective
    /// transaction for which `foreign` is true touched an overlapping path: that op is
    /// skipped (the other participant's later value stands) and returned in
    /// [`Rebased::skipped`]. Overlapping later transactions that are not foreign fail
    /// the undo with `ARA1502`, as in [`undo`](Self::undo). When every op is skipped,
    /// nothing is committed and `id` stays in effect.
    pub fn undo_rebased(
        &mut self,
        id: &TxnId,
        by: Author,
        foreign: &dyn Fn(&Transaction) -> bool,
    ) -> Result<Rebased<'_>, Vec<Diagnostic>> {
        let Some(target) = self.txn(id).cloned() else {
            return Err(vec![Diagnostic::error(
                codes::TXN_NOT_FOUND,
                format!("no transaction `{id}`"),
            )]);
        };
        if let Some(r) = self.reverted_by.get(id) {
            return Err(vec![Diagnostic::error(
                codes::ALREADY_REVERTED,
                format!("`{id}` was already reverted by `{r}`"),
            )]);
        }
        let later: Vec<&Transaction> = self
            .since(target.result)
            .iter()
            .filter(|t| foreign(t) && !self.cancelled(t, target.result))
            .collect();
        let overwritten = |op: &Op| {
            let mine = Touch::of(op);
            later.iter().any(|t| {
                t.ops
                    .iter()
                    .flat_map(Touch::of)
                    .any(|theirs| mine.iter().any(|m| m.overlaps(&theirs)))
            })
        };
        let (mut skipped, mut inverse) = (Vec::new(), Vec::new());
        for op in target.ops.iter().rev() {
            if overwritten(op) {
                skipped.push(op.clone());
            } else {
                inverse.push(op.inverse().to_edit());
            }
        }
        skipped.reverse();
        if inverse.is_empty() {
            return Ok(Rebased { txn: None, skipped });
        }
        let label = format!("undo {}", target.label.as_deref().unwrap_or(&target.id.0));
        let meta = TxnMeta {
            author: by,
            why: None,
            label: Some(label),
            base: Some(target.result),
            idempotency_key: None,
        };
        let mut staged = self.begin(meta);
        staged.reverts = Some(target.id.clone());
        for edit in inverse {
            staged.apply(edit).map_err(|d| vec![d])?;
        }
        staged.commit()?;
        let new = self.history.len() - 1;
        self.reverted_by
            .insert(target.id.clone(), self.history[new].id.clone());
        if let Some(stacks) = self.stacks.get_mut(&target.author) {
            stacks.undo.retain(|t| *t != target.id);
        }
        Ok(Rebased {
            txn: Some(&self.history[new]),
            skipped,
        })
    }

    /// True if `t` (committed after `base`) no longer counts: it was undone, or it is
    /// the undo of a transaction also committed after `base`.
    fn cancelled(&self, t: &Transaction, base: u64) -> bool {
        self.reverted_by.contains_key(&t.id)
            || t.reverts
                .as_ref()
                .and_then(|r| self.txn(r))
                .is_some_and(|r| r.result > base)
    }

    /// Commits the inverse of `id`. Returns the new transaction's history index.
    fn revert(&mut self, id: &TxnId, by: Author) -> Result<usize, Vec<Diagnostic>> {
        let Some(target) = self.txn(id).cloned() else {
            return Err(vec![Diagnostic::error(
                codes::TXN_NOT_FOUND,
                format!("no transaction `{id}`"),
            )]);
        };
        if let Some(r) = self.reverted_by.get(id) {
            return Err(vec![Diagnostic::error(
                codes::ALREADY_REVERTED,
                format!("`{id}` was already reverted by `{r}`"),
            )]);
        }
        let label = format!("undo {}", target.label.as_deref().unwrap_or(&target.id.0));
        let meta = TxnMeta {
            author: by,
            why: None,
            label: Some(label),
            base: Some(target.result),
            idempotency_key: None,
        };
        let mut staged = self.begin(meta);
        staged.reverts = Some(target.id.clone());
        for op in target.ops.iter().rev() {
            staged.apply(op.inverse().to_edit()).map_err(|d| vec![d])?;
        }
        staged.commit()?;
        let new = self.history.len() - 1;
        self.reverted_by
            .insert(target.id, self.history[new].id.clone());
        Ok(new)
    }

    /// Diagnostics for transactions after `base` that touched any of `touched`.
    /// An undone transaction and its undo cancel out and are skipped.
    fn conflicts(&self, base: u64, touched: &[Touch], undo: bool) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for t in self.since(base) {
            if self.cancelled(t, base) {
                continue;
            }
            for op in &t.ops {
                let theirs = Touch::of(op);
                for (mine, theirs) in touched
                    .iter()
                    .flat_map(|m| theirs.iter().map(move |t| (m, t)))
                {
                    if mine.overlaps(theirs) {
                        let (code, verb) = if undo {
                            (codes::UNDO_CONFLICT, "undo would overwrite")
                        } else {
                            (codes::STALE_BASE, "your base revision predates")
                        };
                        let what = theirs
                            .path
                            .as_ref()
                            .map_or("the record".to_owned(), |p| format!("`{p}`"));
                        out.push(
                            Diagnostic::error(
                                code,
                                format!(
                                    "{verb} {what}, changed by `{}` ({}) at revision {}",
                                    t.id, t.author, t.result
                                ),
                            )
                            .doc(theirs.doc.as_str())
                            .record(theirs.record.as_str())
                            .path(
                                theirs
                                    .path
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_default(),
                            ),
                        );
                    }
                }
            }
        }
        out.dedup();
        out
    }

    /// Undoes applied ops, newest first. Used when a transaction fails.
    fn rollback(&mut self, ops: &[Op]) {
        for op in ops.iter().rev() {
            if matches!(op, Op::Document { .. }) {
                self.ws
                    .apply(op.inverse().to_edit(), &self.db)
                    .expect("inverse of a staged document lifecycle op applies");
            } else if let Some(doc) = self.ws.doc_mut(op.doc().as_str()) {
                doc.revert(op, &self.db);
            }
        }
    }

    /// Records a validated transaction and notifies subscribers.
    fn record(&mut self, meta: TxnMeta, reverts: Option<TxnId>, ops: Vec<Op>) -> usize {
        self.rev += 1;
        let id = TxnId(self.ids.token("tx", 8));
        let txn = Transaction {
            id: id.clone(),
            base: meta.base.unwrap_or(self.rev - 1),
            result: self.rev,
            author: meta.author,
            why: meta.why,
            label: meta.label,
            idempotency_key: meta.idempotency_key,
            reverts,
            ops,
        };
        let index = self.history.len();
        if let Some(key) = &txn.idempotency_key {
            self.by_key.insert(key.clone(), index);
        }
        self.record_stacks(&txn);
        self.by_id.insert(id, index);
        self.subscribers.retain(|s| s.send(txn.clone()).is_ok());
        self.history.push(txn);
        index
    }

    /// Rebuildable from the journal's transactions, including selective undo.
    fn record_stacks(&mut self, txn: &Transaction) {
        if let Some(target) = &txn.reverts {
            let redo = self
                .stacks
                .get(&txn.author)
                .is_some_and(|s| s.redo.contains(target));
            for stacks in self.stacks.values_mut() {
                stacks.undo.retain(|id| id != target);
                stacks.redo.retain(|id| id != target);
            }
            let stacks = self.stacks.entry(txn.author.clone()).or_default();
            if redo {
                stacks.undo.push(txn.id.clone());
            } else {
                stacks.redo.push(txn.id.clone());
            }
            self.reverted_by.insert(target.clone(), txn.id.clone());
        } else {
            let stacks = self.stacks.entry(txn.author.clone()).or_default();
            stacks.undo.push(txn.id.clone());
            stacks.redo.clear();
        }
    }
}

fn nothing(author: &Author, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::NOTHING_TO_UNDO,
        format!("{author} has nothing to {what}"),
    )
}

/// A record path an op touched (`path: None` is the whole record).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Touch {
    doc: AssetId,
    record: RecordId,
    path: Option<FieldPath>,
}

impl Touch {
    fn of(op: &Op) -> Vec<Touch> {
        op.touches()
            .into_iter()
            .map(|path| Touch {
                doc: op.doc().clone(),
                record: op.id().clone(),
                path,
            })
            .collect()
    }

    fn overlaps(&self, other: &Touch) -> bool {
        self.doc == other.doc
            && (self.record == other.record
                || self.record.as_str() == "d_document"
                || other.record.as_str() == "d_document")
            && match (&self.path, &other.path) {
                (Some(a), Some(b)) => a.overlaps(b),
                _ => true,
            }
    }
}

/// An open transaction. Edits apply immediately; dropping it without committing
/// rolls everything back.
///
/// Only errors the transaction introduces fail it. While every edit is
/// [local](Edit::is_local) to records of one document (an Inspector field edit),
/// those records are all it can change, so it validates just them
/// ([`Workspace::check_local`]) instead of whole documents.
#[derive(Debug)]
pub struct Staged<'s> {
    store: Option<&'s mut Store>,
    meta: TxnMeta,
    reverts: Option<TxnId>,
    ops: Vec<Op>,
    /// Errors each touched document had before the transaction, for whole-document
    /// validation.
    baselines: BTreeMap<AssetId, BTreeSet<String>>,
    scope: Scope,
    /// The ID generator at `begin`, restored on rollback so a dry run previews exactly
    /// the IDs a real run mints.
    ids_before: IdGen,
}

/// How a transaction validates.
#[derive(Debug)]
enum Scope {
    /// Every edit so far is local to one document (`None` before the first edit).
    Local(Option<Local>),
    /// Whole touched documents, against `baselines`.
    Whole,
}

/// The records local edits touched in one document, each with the errors it had
/// before the transaction.
#[derive(Debug)]
struct Local {
    doc: AssetId,
    records: BTreeMap<RecordId, BTreeSet<String>>,
}

impl Local {
    fn ids(&self) -> BTreeSet<RecordId> {
        self.records.keys().cloned().collect()
    }
}

impl<'s> Staged<'s> {
    fn store_mut(&mut self) -> &mut Store {
        self.store
            .as_deref_mut()
            .expect("a staged transaction holds its store until commit or rollback")
    }

    /// The store, as it looks with this transaction's edits applied.
    pub fn store(&self) -> &Store {
        self.store
            .as_deref()
            .expect("a staged transaction holds its store until commit or rollback")
    }

    /// The ops applied so far.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// The ID generator.
    pub fn ids(&mut self) -> &mut IdGen {
        &mut self.store_mut().ids
    }

    /// Applies one edit now. On error nothing changes and the transaction stays open.
    pub fn apply(&mut self, edit: Edit) -> Result<&Op, Diagnostic> {
        let doc_id = edit.doc().clone();
        let touched = self.baselines.contains_key(&doc_id)
            || matches!(&self.scope, Scope::Local(Some(local)) if local.doc == doc_id);
        if !touched
            && self.store().ws.doc(doc_id.as_str()).is_none()
            && !matches!(edit, Edit::Document { kind: Some(_), .. })
        {
            return Err(Diagnostic::error(
                codes::DOC_NOT_FOUND,
                format!("no document `{doc_id}` is loaded"),
            ));
        }
        match (&mut self.scope, &edit) {
            (Scope::Local(local), Edit::Set { id, .. })
                if edit.is_local() && local.as_ref().is_none_or(|l| l.doc == doc_id) =>
            {
                let local = local.get_or_insert_with(|| Local {
                    doc: doc_id.clone(),
                    records: BTreeMap::new(),
                });
                if !local.records.contains_key(id) {
                    let store = self
                        .store
                        .as_deref()
                        .expect("a staged transaction holds its store until commit or rollback");
                    let ids = BTreeSet::from([id.clone()]);
                    let before = store.ws.check_local(&doc_id, &ids, &store.db);
                    local.records.insert(id.clone(), error_keys(&before));
                }
            }
            _ => {
                self.validate_whole();
                if !self.baselines.contains_key(&doc_id) {
                    let store = self.store();
                    let baseline = error_keys(&store.ws.check(&doc_id, None, &store.db));
                    self.baselines.insert(doc_id.clone(), baseline);
                }
            }
        }
        let store = self.store_mut();
        let op = store.ws.apply(edit, &store.db)?;
        let index = self.ops.len();
        self.ops.push(op);
        Ok(&self.ops[index])
    }

    /// Switches to whole-document validation. The document local edits touched gets
    /// the baseline it had before the transaction: its errors now, with its touched
    /// records' own errors put back to what they were before their first edit (local
    /// edits change nothing else).
    fn validate_whole(&mut self) {
        let Scope::Local(local) = std::mem::replace(&mut self.scope, Scope::Whole) else {
            return;
        };
        let Some(local) = local else {
            return;
        };
        let store = self.store();
        let ids = local.ids();
        let now = error_keys(&store.ws.check(&local.doc, None, &store.db));
        let edited = error_keys(&store.ws.check_local(&local.doc, &ids, &store.db));
        let mut baseline: BTreeSet<String> = now.difference(&edited).cloned().collect();
        baseline.extend(local.records.into_values().flatten());
        self.baselines.insert(local.doc, baseline);
    }

    /// The errors committing now would report (stale base, undo conflicts, new
    /// validation errors), without committing. Dry runs use this.
    pub fn check(&self) -> Vec<Diagnostic> {
        let store = self.store();
        let mut errors = Vec::new();
        if let Some(base) = self.meta.base {
            if base > store.rev {
                errors.push(Diagnostic::error(
                    codes::STALE_BASE,
                    format!("base revision {base} is ahead of the store ({})", store.rev),
                ));
            } else {
                let touched: Vec<Touch> = self.ops.iter().flat_map(Touch::of).collect();
                errors.extend(store.conflicts(base, &touched, self.reverts.is_some()));
            }
        }
        if let Some(guard) = &store.guard {
            errors.extend(guard.check(&self.meta, store.origin(), &self.ops));
        }
        if let Scope::Local(Some(local)) = &self.scope {
            let before: BTreeSet<&String> = local.records.values().flatten().collect();
            let after = store.ws.check_local(&local.doc, &local.ids(), &store.db);
            errors.extend(
                after
                    .into_iter()
                    .filter(|d| d.is_error() && !before.contains(&key_of(d))),
            );
        }
        for (doc, before) in &self.baselines {
            let after = store.ws.check(doc, None, &store.db);
            errors.extend(
                after
                    .into_iter()
                    .filter(|d| d.is_error() && !before.contains(&key_of(d))),
            );
        }
        errors
    }

    /// Validates every touched document and records the transaction, or rolls it
    /// back and returns the errors it introduced.
    pub fn commit(mut self) -> Result<Commit<'s>, Vec<Diagnostic>> {
        let replayed = self
            .meta
            .idempotency_key
            .as_ref()
            .and_then(|k| self.store().by_key.get(k).copied());
        let errors = if replayed.is_some() || self.ops.is_empty() {
            Vec::new()
        } else {
            self.check()
        };
        let ops = std::mem::take(&mut self.ops);
        let ids = self.ids_before.clone();
        let store = self
            .store
            .take()
            .expect("a staged transaction holds its store until commit or rollback");
        if let Some(i) = replayed {
            abort(store, &ops, ids);
            return Ok(Commit::Replayed(&store.history[i]));
        }
        if ops.is_empty() {
            store.ids = ids;
            return Ok(Commit::Empty);
        }
        if !errors.is_empty() {
            abort(store, &ops, ids);
            return Err(errors);
        }
        let index = store.record(self.meta.clone(), self.reverts.take(), ops);
        Ok(Commit::Applied(&store.history[index]))
    }

    /// Rolls back and returns the ops that were applied: a dry run's diff.
    pub fn rollback(mut self) -> Vec<Op> {
        let ops = std::mem::take(&mut self.ops);
        if let Some(store) = self.store.take() {
            abort(store, &ops, self.ids_before.clone());
        }
        ops
    }
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        let ops = std::mem::take(&mut self.ops);
        if let Some(store) = self.store.take() {
            abort(store, &ops, self.ids_before.clone());
        }
    }
}

/// Rolls back `ops` and restores the ID generator.
fn abort(store: &mut Store, ops: &[Op], ids: IdGen) {
    store.rollback(ops);
    store.ids = ids;
}

fn key_of(d: &Diagnostic) -> String {
    format!("{}|{}|{}", d.code, d.location, d.message)
}

fn error_keys(diags: &[Diagnostic]) -> BTreeSet<String> {
    diags.iter().filter(|d| d.is_error()).map(key_of).collect()
}
