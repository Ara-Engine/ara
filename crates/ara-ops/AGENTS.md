# ara-ops

Transactions over documents (decision A4): all-or-nothing apply after validation,
attribution, base/result revisions, idempotency, undo/redo with conflict detection,
and the op stream for the Engine Protocol. Depends on ara-doc, ara-schema, ara-diag.

## Public API

- `Store::new(db, workspace)` / `Store::with_ids(db, workspace, IdGen::seeded(n))`.
  Reads: `schema()`, `workspace()`, `revision()`, `history()`, `since(rev)`,
  `txn(id)`, `reverted_by(id)`, `find_by_key(key)`. `schema_mut()` for runtime types.
- `store.begin(TxnMeta) -> Staged`: `apply(Edit) -> Result<&Op, Diagnostic>` (takes
  effect immediately), `store()`, `ops()`, `ids()`, then `commit()` or `rollback()`
  (returns the ops: a dry run's diff). Dropping a `Staged` rolls back.
- `store.transact(meta, edits) -> Result<Commit, Vec<Diagnostic>>`.
- `Commit::{Applied(&Transaction), Replayed(&Transaction), Empty}`.
- `TxnMeta { author, why, label, base, idempotency_key }`, `TxnMeta::by(author)`.
- `Transaction { id, author, why, label, base, result, idempotency_key, reverts, ops }`
  (serde: camelCase; ops tagged `"op": "set"`).
- `Author::{Human, Agent(name)}` ↔ `"human"` / `"agent:<name>"`.
- Undo: `undo(txn, by)` (selective, e.g. review revert), `undo_last(author)`,
  `redo_last(author)`.
- `subscribe() -> mpsc::Receiver<Transaction>`.

## Semantics

- **Commit** validates every touched document fully (records, parents, loops,
  prefab instances) and fails only on errors the transaction introduced; errors the
  document already had don't block edits. On failure every op is rolled back.
- **Base revision**: with `base: Some(r)` and later transactions touching
  overlapping paths (prefix-related field paths, or the whole record for
  create/delete), the commit fails with `ARA1501`. Disjoint paths merge fine.
- **Undo** applies inverse ops as a new transaction with `reverts` set, based on the
  target's result revision: a later transaction touching the same paths makes it
  fail with `ARA1502`, unless that transaction was itself undone (a transaction and
  its undo cancel out). Redo is undoing the undo. Per-author undo/redo stacks; a new
  edit clears the author's redo stack.
- **Idempotency**: a repeated key returns `Commit::Replayed` with the original.
- Empty transactions record nothing.
- IDs: transactions get `tx_` + 8 base32 chars from the store's `IdGen`.

## Gotchas

- Commit-time validation is a full check of each touched document (O(document)),
  twice per transaction (baseline + after). Fine for authoring sizes; make it
  incremental before huge levels.
- History is unbounded in memory; there is no journal on disk yet.
- Review state (accepted/unreviewed) is not tracked yet; `reverts` and
  `reverted_by` cover "reverted".

## Testing

`cargo test -p ara-ops`: atomicity, attribution and streaming, idempotency, stale
bases, undo conflicts and cancellation, per-author stacks, and a property test that
undoing everything restores the start and redoing everything restores the end.
