# ara-collab

Collaboration foundations (D21): many humans and agents in one project and scene,
live. Presence, claims (soft locks), live op sync with field-level last-writer-wins,
per-participant undo, replicas, late join, agent branch sessions (D20) and the
editor seam. Pure logic: no IO, no clock (`now` in ms is a parameter). Served by
`ara-host`'s `collab` module. Depends on ara-ops, ara-doc, ara-merge, ara-schema,
ara-json, ara-diag.

## Public API

- `Hub::new(revision, ids)`: the authority's state next to its `Store`.
  `join(id, author, name, color, now)`, `leave`, `disconnect` (claims stay),
  `set_presence` (also a claim heartbeat), `claim(store, id, Vec<Wanted>, ttl, why,
  now) -> Granted`, `release`, `heartbeat`, `expire`, `tick` (expiry, coalesced
  presence, policy merges), `submit(store, id, Submit, now) -> Ack`,
  `publish(store, attribution, now)` (after every call: `collab.op` events, undo
  stacks, activity), `replay(store, since)`, `undo/redo(store, id) ->
  UndoOutcome`, `who(store, me, now)`, `drain_events() -> Vec<Event {to, method,
  params}>`, `set_policy(ApplyPolicy, idle_ms)`, `claims_version()`.
- Branches: `branch_open`, `branch_store` (owner or same agent author; open only),
  `branch_submit`, `branch_merge(store, approver, id, Prefer, force)` (humans),
  `branch_discard`, `take_dirty_branches() -> Vec<SavedBranch>`,
  `restore_branch`. `Branch { info(), changes(), base_doc(), store(), save(),
  load() }`.
- `Replica::new/from_snapshot`, `edit(edits, why, label) -> Submit`,
  `receive(&OpEvent)`, `rejected(seq)`, `resubmit()`, `resync(&Snapshot)`,
  `workspace()`, `revision()`, `pending()`.
- `Snapshot::of(store)` / `workspace(db)`; `Subject` (`Doc`, `Entity`, `Field`,
  `Asset`) with `overlaps` / `touched_by(op)`; `ClaimGuard` (an `ara_ops::Guard`).
- Wire types (`sync`): `OpEvent {participant?, seq?, base, txn}`, `Submit`,
  `Reject`, `Conflict {kind: overwritten|dropped|merge, loser, winner, …}`,
  notification names in `sync::methods`. Codes: `codes` (ARA2500–ARA2512, docs in
  `ara-diag/codes/collab.md`).

## Rules

- Participant = host session. Claims bind per session (store origin); with no
  origin, per author. Humans are never blocked; agents get ARA2500 (commit and dry
  run) naming the claimant with fixes (`collab.who`, `branch.open`).
- The authority applies a submit with `base: None` on top of everything since its
  base; the same field is last-writer-wins by arrival order, the overwritten value
  goes to the conflict log and to its author. A submit that no longer applies is
  dropped whole (atomic) and answered with `collab.reject`.
- Replica = authority state at `revision` plus pending changes on top; op events
  rewind pending, apply, re-apply. Nothing pending ⇒ byte-identical documents.
- Undo is per participant: `Store::undo_rebased` skips paths someone else changed
  later. Redo undoes the undo.
- Branch merge: three-way per touched document (base = the fork, reconstructed by
  reverting the branch's history); conflicts keep live unless `prefer: branch`;
  lands as one transaction by the branch author (`branch.merge br_…`). Submitted
  branches merge per `ApplyPolicy` once no one else claims what they touch.

## Editor seam (`view::CollabView`; no UI here)

Feed it `collab.join`'s result (`join`) and every `collab.*` notification
(`apply(method, params) -> changed`). What the editor should render:

- **Hierarchy:** `avatars(doc_path, address)` → coloured dots (agents: robot glyph)
  on rows others have selected; `badge(doc_id, address)` → a lock badge in the
  claimant's colour, tooltip `label`, `why`, time left; red when `blocks_me`.
- **Viewport:** `cameras(doc_path)` → frustum gizmos with names, pointer dots;
  selection outlines in the avatar colour; `follow(Some(id))` +
  `follow_camera()` drives the camera (send `following` in presence too).
- **Inspector:** `claims_in(doc_id)` → header banner per claimed record/component.
- **Conflict log panel:** `my_conflicts()` (kind, field, my value vs the winner's,
  who) with "re-apply mine".
- **Agents panel:** `branches()` rows (status, why, waiting on) with
  approve (`branch.merge`), discard, and the apply policy (`collab.policy`).
- Send `collab.presence` on camera/selection change (the host rate-limits to
  20 Hz per participant); replicas use `Replica` + `collab.submit`.

## Testing and numbers

`cargo test -p ara-collab`: unit (subjects, presence coalescing), `tests/hub.rs`
(claims, LWW + log, drops, undo, late join, branches, approval, who),
`tests/convergence.rs` (proptest: 3–5 replicas, random edits incl. create, delete
and move, random FIFO interleavings, 96 cases; plus a 2000-step run: 484 commits,
193 overwritten, 63 dropped, converged), `tests/demo.rs` (two processes, below).

Demo (Windows dev box, debug-opt1, 2 human replicas × 100 edits + presence, 2
agents, 1 observer, 9.4 s): converged; authority time per `collab.submit` p50
0.45 / p95 1.5 ms, `cmd.apply` 0.8 / 2.5 ms; round trip p50 30.6 / p95 33.9 ms and
op → other client p50 17–21 / p95 33–46 ms, dominated by ara-rpc's 20 ms socket
poll; human replicas ≈ 4.5 kB/s up, 17 kB/s down; presence 100 sent → ≤ 103
broadcast (20 Hz cap with activity changes).

## Gotchas

- The claim guard is a snapshot reinstalled on changes and at most every second
  (time-left text); expiry is enforced by `expire`/`tick` and in the guard.
- Asset subjects are advisory until the asset and code pipelines check claims.
- `load_doc`/`unload_doc` (files appearing/disappearing) are not ops; replicas
  must resync (`collab.sync {snapshot: true}`) when the document set changes.
- History and the origin map are unbounded, like `Store` history.
