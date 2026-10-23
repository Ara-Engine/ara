# Collaboration: presence, claims, live sync, branch sessions (ARA2500–ARA2599)

## ARA2500 collab::CLAIMED: Subject claimed by someone else
owner: ara-collab

Retired by D24. Agent changes apply immediately; advisory claims never reject edits or branch merges. This code is no longer emitted.

Causes:
- An older engine build enforced claims.

Fixes:
- Update the engine; edits need no approval or claim release.

## ARA2501 collab::CLAIM_HELD: Claim already held
owner: ara-collab

Retired by D24. Agent changes apply immediately; advisory claims never reject edits or branch merges. This code is no longer emitted.

Causes:
- An older engine build enforced claims.

Fixes:
- Update the engine; edits need no approval or claim release.

## ARA2502 collab::NOT_JOINED: Not joined
owner: ara-collab

The session called a collaboration method that needs a participant (presence, replica submits, per-participant undo, branch approval) without joining first.

Causes:
- `collab.join` was never called for this session, or the session left.

Fixes:
- Call `collab.join` with the session handle, then retry.

## ARA2503 collab::OVERWRITTEN: Change overwritten by a concurrent edit
owner: ara-collab

A transaction overwrote a field another participant changed recently (default ten minutes, configurable in HostConfig). The authority applies changes in arrival order and the later one wins (last writer wins). The earlier change is kept in the conflict log (`collab.conflicts`) with both values and surfaced to both participants through diagnostics and editor.overwrite.

Causes:
- Two people (or a person and an agent) edited the same field at the same time.

Fixes:
- Look at the conflict log entry and set the field again if your value should win.
- Undo the agent turn if the previous value should be restored.

## ARA2504 collab::DROPPED: Change dropped
owner: ara-collab

A replica's optimistic change no longer applied at the authority, usually because a concurrent change deleted or restructured what it edited. Transactions are all or nothing, so the whole change was dropped, logged in the conflict log and answered with `collab.reject`; the replica removes it from its view.

Causes:
- The record was deleted, or its parent removed, by someone else just before the change arrived.

Fixes:
- Read the conflict log entry, then redo the change on what exists now.

## ARA2505 collab::BRANCH_NOT_FOUND: No such branch session
owner: ara-collab

No branch session has this ID.

Causes:
- A typo in the ID, or the branch belongs to another project or host.

Fixes:
- List branch sessions with `branch.list` and use one of their IDs.

## ARA2506 collab::BRANCH_STATE: Branch session in the wrong state
owner: ara-collab

The branch session cannot do this now: only its owner edits it, only open branches take edits and submits, and merged or discarded branches are final.

Causes:
- Editing a branch after `branch.submit`, or someone else's branch.
- Merging or discarding a branch twice.

Fixes:
- Open a new branch session with `branch.open` for further work.

## ARA2507 collab::MERGE_KEPT_LIVE: Branch merge kept the live value
owner: ara-collab

A branch merge found fields that both the live documents and the branch changed since the branch was forked. The live value was kept (the human's work is never overwritten) unless the merge asked to prefer the branch; every such field is in the merge report and the conflict log.

Causes:
- Someone edited the same field live while the agent worked in its branch.

Fixes:
- Review the merge report and set the fields again if the branch's values should win, or merge with `prefer: branch`.

## ARA2508 collab::REPLICA_GAP: Replica out of sync
owner: ara-collab

A replica received an op event that does not follow its revision, or one that does not apply to its state. Applying it would diverge, so the replica stopped and needs a resync.

Causes:
- Notifications were lost (a dropped connection) or delivered out of order.

Fixes:
- Call `collab.sync` with the replica's revision to replay the missed ops, or with `snapshot: true` to reload, then resubmit pending changes.

## ARA2509 collab::UNDO_PARTIAL: Undo left others' changes alone
owner: ara-collab

Per-participant undo reverts only your own change. Fields someone else changed after you keep their value; the undo result lists them under `skipped` with who changed them.

Causes:
- Another participant edited the same field after your change.

Fixes:
- Nothing, if their change should stand; otherwise set the field yourself.

## ARA2510 collab::INVALID_SUBJECT: Invalid claim subject
owner: ara-collab

A claim subject does not name a loaded document, an entity in it, or a project path.

Causes:
- A typo in the entity ID or document path, or an entity that was deleted.

Fixes:
- Use a document path (`levels/outpost.scene`), an entity ID or address with `doc`, `<entity>.<Component>` for a component, or an `@path` for other files.

## ARA2511 collab::HUMAN_ONLY: Human sessions only
owner: ara-collab

Only humans set the apply policy for agent branch sessions and approve branch merges (D20: the human decides when agent work lands).

Causes:
- An agent session called `collab.policy` or `branch.merge`.

Fixes:
- Submit the branch with `branch.submit`; a human merges it, or the apply policy does.

## ARA2512 collab::MERGE_BLOCKED: Branch merge waits for claims
owner: ara-collab

Retired by D24. Agent changes apply immediately; advisory claims never reject edits or branch merges. This code is no longer emitted.

Causes:
- An older engine build enforced claims.

Fixes:
- Update the engine; edits need no approval or claim release.
