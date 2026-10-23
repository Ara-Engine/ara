# ara-collab

Pure collaboration logic: presence, attributed op sync, field-level last writer
wins, conflict log, per-participant undo, replicas, snapshots and optional branch
sessions. No IO or wall clock; the caller supplies `now` in milliseconds.

## Public API

`Hub::new(revision, ids)`: `join/leave/disconnect`, `set_presence`, `submit`,
`publish`, `replay`, `undo/redo`, `who`, `drain_events`. `set_overwrite_window(ms)`
sets the recent-field notice window (default ten minutes). `publish` detects
ordinary command overwrites as well as replica conflicts. Both participants get
`collab.conflict`; `editor.overwrite` lets the host log ARA2503 and notify editors.
The losing value stays in `ConflictLog`. Same-participant and same-value writes
are silent; old fields age out. Recent writes are ephemeral across a restart.

`Replica` maintains optimistic pending ops and rebases on authority events.
`Snapshot::of(store)` produces a late-join document snapshot.
`Subject` parses advisory subjects. `Claim` / claim/release/heartbeat APIs remain
for legacy clients but never install an ops guard or reject any participant.
`ApplyPolicy` and ClaimGuard were removed by D24. ARA2500/2501/2512 are retired.

Branches: `branch_open/store/submit/merge/discard`, `take_dirty_branches`,
`restore_branch`. Branches are optional isolation; submit merges immediately,
without approval/idle/claim gates. Manual branch.merge is human-only; `force` is
a compatibility no-op. Three-way conflicts keep live unless prefer: branch.

`view::CollabView` consumes join state and notifications for remote avatars,
cameras, selection, follow mode, conflicts and branch rows. Legacy claim badges
are advisory and `blocks_me` is always false. Agents activity needs no accept UI.

## Testing

`cargo test -p ara-collab`: same-field LWW/conflict log, independent fields,
per-participant undo, optional branch isolation/merge, proptest convergence and
the two-process collaboration demo (now immediate-application semantics).
`cargo clippy -p ara-collab --all-targets -- -D warnings`.
Host-facing coverage lives in ara-host/tests/collab.rs, including command
notices and exact-field human gesture protection.
