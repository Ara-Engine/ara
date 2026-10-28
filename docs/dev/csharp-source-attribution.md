# C# source attribution

`script.write` prepares the replacement's SHA-256 on its worker, registers an
engine-private receipt `{edit, hash, phase: "preparing"}` with each running
compiler, and applies the ordinary attributed `source.replace` transaction.
The edit identity is the source job ID; the hash covers exact UTF-8 bytes,
including a BOM and original line endings. Canonical project paths identify files.

Pending receipts have no elapsed-time expiry. The compiler compares a settled
watch event's actual bytes with pending revisions and the last known agent write
echo. Matching bytes retain agent origin; unrelated bytes are human work. A mixed
human/agent watch batch is human work. These are attribution receipts, not locks:
human saves remain immediate and retain D20 scheduling priority.
Directory notices remain in the compile work set for module/editor-boundary
validation, but their metadata echoes do not change source-byte attribution.
The watcher's shared input predicate identifies `.cs` and module manifests.

The source job reports `materialized` only after the host's file worker finishes.
That retires only the matching edit and verifies its current hash before asking
for a compile (also covering a missed watcher notification). Failed/cancelled jobs
report `cancelled`. If commit already made a child job noncancellable, its receipt
survives parent cancellation until materialization actually finishes. A matching
write echo remains recognizable; the next different observed revision clears it,
so a subsequent human revert to old agent bytes is still human work.

The receipt travels as additive private metadata over the existing compile
request and compile-server pipe. No C# SDK, registry, MCP or native ABI changes.
Hashing and filesystem reads happen on preparation/compiler workers, not ticks.
Immediate `request_compile` calls concern current bytes; callers preparing future
bytes use the revision receipt instead of claiming a path for a timed window.

`SourceClaimTests` controls the scheduler clock and delivers settled watcher work
directly. The old two-second policy deterministically reported human origin after
a three-second advance before writing; the revision receipt preserves agent origin.
The tests also cover unrelated human saves while pending, duplicate write echoes,
later human reverts, directory echoes, genuinely mixed human/agent source batches,
and cancelling one of two outstanding revisions. The Windows
C# Play test exercises the actual source job, transaction, materialization and reload.
