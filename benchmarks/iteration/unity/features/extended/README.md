# Extended native ECS feature fixtures

Status: all 50 planned paired serial/parallel smokes passed exact full-state/oracle
agreement. Standalone release build, release Clippy and 38 mocked Python contracts
passed. Actual C# compilation, normal checkpoint/finally and explicit known-state
recovery gates passed too. [Durable evidence](../extended_smoke_mac_2026-11-09.json)
labels 22 pre-checkpoint records and 28 checkpointed records separately; older
records retain their original controllers and proofs. Runtime kernels and the
Bevy executable are identical across those accepted smoke pairs. Their performance
timings remain unaccepted. Query case 1 and its old quality artifact remain unchanged.
New [phase A timing evidence](../feature_timed_phase_a_mac_2026-11-09.json)
separately covers12 first-order pairs at100k entities/420steps with full independent
reference agreement and strict resource/Burst/restoration controls. Six meet the
target; parallel dirty/DAG/four-word buffers and serial buffers lose to Unity,
while serial DAG and64-archetype query miss the20% threshold. Remaining24 pairs,
including resize/64-word buffers/reversed confirmation, are pending; see the
[current timing table](../README.md). The original50-smoke evidence is preserved.
The first Phase B Unity invocation subsequently crashed in native Burst compilation.
Its timings are excluded; exact known-checkpoint recovery succeeded and the
global guard is clear. [Recovery evidence](../feature_timed_phase_b_recovery_mac_2026-11-09.json)
preserves the crash/receipt/control proofs. No automatic benchmark retry ran;
remaining24 pairs are still pending.
Cases 2–6 follow [CONTRACT.md](../CONTRACT.md) and the existing independent
[integer oracle](../reference.py). These fixtures exercise native Bevy/Unity APIs;
they do not establish Ara public C# buffer or JobHandle availability, shipping
player performance, Unreal parity, or an all-engine win.

| Case | Retained implementation | Work inside each timed window |
|---|---|---|
| Dirty epoch | Bevy `Changed<Epoch>` systems; Unity registered changed-version query and `IJobChunk` marker | Read selection IDs, write selected epochs, dispatch mark/consumer dependency, explicit epoch guard, payload/update count, final join |
| Archetype churn | Bevy per-batch `Commands`/`CommandQueue`; Unity ECB parallel writer and post-playback TagD query | Record real typed commands, join, deterministic ascending-ID playback, structural movement, query maintenance, payload/update count, completion; Unity ECB creation/disposal included |
| Dependency DAG | Bevy retained `Schedule`; Unity retained query and four Burst jobs | Four actual component-column stages, chain or fork/join dependencies, dependency combination and final completion |
| Fixed buffers | Component-owned `SmallVec<[u32; 4]>`; real `DynamicBuffer<Word>` with inline capacity four | Every live word update and wrapping sum into entity payload; zero-length entities still increment their update count |
| Resize buffers | Same owned ECS buffers | Real growth/shrink, new-word initialization, optional explicit trim, live-word update/fold and bookkeeping |

Bevy structural queues are retained and recorded in parallel batches, then applied
in stable batch/ID order. Unity records ECB sort keys using stable selection
indices; recording and playback storage strategies differ. No matching set is
reconstructed by reading a raw external payload array. The immutable selected
entity-handle list is setup infrastructure in both engines. Bevy's default table
storage is used for TagD. Its direct `CommandQueue::apply` is the explicit deferred
boundary; there is no untimed playback stage.

The single-runner Unity jobs still use scheduling and `Complete`, while direct
Bevy churn/buffer query work runs on its caller. Bevy dirty/DAG use a real
single-threaded `Schedule` in that lane. These dispatch paths have different
overhead, and that overhead stays timed. Explicit `--batch` controls Bevy command
recording chunks only; Bevy parallel queries use their normal default strategy,
and Unity ECB recording uses batch 64. No identical partitioning is claimed.

For 15 potential runners, direct Bevy queries/scopes configure 14 pool workers
plus an eligible caller. Send-only retained Bevy schedules configure 15 workers;
the installed 0.20.0 executor enters `scope_with_executor(false, ...)`. Unity
configures 14 job workers plus its completing caller. Metadata separately records
configured pools, theoretical ceilings and a small untimed worker probe. Bevy's
probe records observed thread IDs and whether its caller appeared; Unity reports
injected native thread slots without pretending they identify the managed caller.
Neither observation is proof of simultaneous activity in the measured case. The
first real gate must inspect these results and executor semantics before timing.

Full export uses the unchanged version-one format with case IDs 2–6, exact
unique IDs, actual component/tag presence, epochs, A/B/C, update counts, lengths
and every live word. Export rejects wrong identity, presence or operation counts.
Smoke verification additionally recomputes the independent scalar oracle for
every case/variant and requires a separately generated reference file. Reuse and
trim, and chain and fork/join, intentionally have identical logical bytes but
different immutable inputs. Launch evidence binds the exact config JSON bytes.

Dirty candidate visits are counted only in a separate single-threaded replay of
the real change filters. That replay must export/hash-match measured full state;
its step-by-step logical counts must agree across both engines and the oracle.
Creation may make all rows candidates on the first tick even for zero dirty
percent; chunk candidates may exceed explicit logical matches. The marker first
reads IDs and only obtains writable Unity epoch arrays for selected chunks, so
unselected chunks are not dirtied merely by requesting their mutable arrays.
These counters mean returned consumer candidates, not total rows scanned by the
marker or the filter implementation.

Buffer observations report actual component size/alignment, inline capacity,
live words, capacity histograms and SmallVec `spilled()` values. Unity reports
capacity above inline rather than an unobserved pointer/allocation count.
Installed SmallVec 1.16.5 `shrink_to_fit` moves fitting buffers back inline;
installed Unity `TrimExcess` does likewise. The six-step resize smoke checks the
short state after growth/shrink; the five-step smoke checks grown tail contents.
Actual post-run observations must confirm this before a physical-storage claim.
Resize exporters now assert the selected policy after at least two steps: a
short explicit-trim buffer must have inline capacity (and SmallVec must report
not spilled); a grown or capacity-reuse buffer must retain external capacity
(and SmallVec must report spilled). Component size/alignment uses actual runtime
type layout, including nonbuffer columns, rather than hardcoded reporting sizes.

The new default scratch project is `target/perf-scale/ufx`, new target is
`target/perf-scale/extended-feature-rust`, and executable is
`ara-ecs-extended-fixture`. Existing query outputs in `uf` and `feature-rust` are
preserved. All inputs/results/logs/dumps remain under `target/`. Bevy caret ranges
stay in the fixture manifest; its existing lock remains production-aligned at
0.20.0. Only an already-locked SmallVec 1.16.5 direct edge was added. No root
dependency or production engine API changed.

First inspect, without launching:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py build-command --case dirty_epoch
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/smoke_plan.py --prefix extended-quality-01
```

The printed plan contains 50 paired serial/parallel smokes across every stated
variant, including both resize parities. It is commands only, never an automatic
execution loop. Use `--case` to inspect one bounded group. In a separately granted
CPU slot, format this standalone crate, run the focused Python tests, then build:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py build-bevy --case dirty_epoch
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py prepare --case dirty_epoch
```

Use an initial `run-unity --case dirty_epoch --smoke --count 1003 --warmup 2
--frames 3 --run extended-bootstrap-01` only for private import/bootstrap. Generated
metadata/settings are unaccepted; re-run `prepare --case dirty_epoch`, then the
actual plan's fresh engine pairs and independent references. Fix real API/source
generation issues, reprepare/rebuild affected sources, and repeat actual smokes.
Run the standalone release Clippy gate and Python source-integrity tests before
calling the packet verified. Smoke always reports `timings_accepted=false` and
`timing_comparable=false`, regardless of idle CPU. No default timed run is
authorized by this source packet; a later quiet two-round matrix needs its own
slot after all relevant variant oracles and worker/storage checks pass.

## Crash checkpoints and explicit recovery

One Unity process exited with native signal 11 in Burst's compiler/cache path;
the historical wrapper cannot prove whether its execute method had entered.
A single identical fresh-process retry passed. Its five captured preference keys,
including presence and values, exactly matched the last successful serial run;
no guessed/manual restoration was performed. Earlier valid runs retain their
original source/manifest provenance. Raw audit is in the owned scratch project's
`crash-preference-audit.json`; the crash attempt is never quality evidence.

Feature wrappers add [PreferenceRecovery.cs](../editor/PreferenceRecovery.cs)
through [recovery.py](../recovery.py), while the frozen S8 wrapper stays unchanged.
Before any benchmark option setter, the helper writes the complete captured
initial JSON/options and exactly five known preferences to a new checkpoint,
flushes file data with `Flush(true)`, then atomically renames in the same directory.
Every finally setter is guarded if checkpoint creation failed. Completion is a
separate receipt bound to checkpoint bytes, initial-state hash, invocation ID,
owned project and result path. This covers native-process failure; it does not
claim filesystem durability against an OS/power failure beyond those operations.

Preparation registers owned feature projects under
`target/perf-scale/feature-preference-recovery`. Every later Unity launch checks
all registered projects for pending failures and unresolved checkpoints, so a
failure in `ufx` also blocks a query run in `uf`. A pre-checkpoint crash is recorded
as an unknown stage, and never invents a recovery baseline. A failed checkpoint
remains available if the launcher itself stops before writing a failure marker.

Recovery is explicit and batch-only, validates the exact checkpoint/whitelist,
restores only captured option values plus known preference values/presence, writes
its own receipt, and exits without a benchmark. It clears only the matching
pending invocation and recovery attempt; unrelated unresolved projects stay
blocked. A fresh attempt records PENDING before launch and produces separate
`.recovery-<32hex>.json/.log/.launch.json` files. An older normal receipt cannot
resolve that attempt. Failed attempt records remain nested in subsequent pending
evidence. The recovery command, requiring a reviewed known checkpoint, is:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py recover-unity \
  --case dirty_epoch --project target/perf-scale/ufx \
  --checkpoint target/perf-scale/ufx/results/RUN/RESULT.json.environment.json.initial.json
```

The actual idempotent gate restored only the exact state captured by a successful
64-ID normal smoke; that smoke's 2,328 full-state bytes also match the oracle.
The first recovery launch failed before its execute method because its temporary
directory made a macOS IPC socket exceed 104 characters. Its log and pending
attempt remain recorded; a reviewed short `ufx-t`/`uf-t` path and fresh attempt
then passed option/key restoration, attempt-bound receipt and global-guard clear.
No historical or guessed preference state was applied. Both failed Burst/socket
attempts are excluded from quality records; the artifact binds raw logs by hash
without publishing incidental log contents. Mocked tests cover
native crash, missing checkpoint/write failure, cross-project blocking, exact
whitelist and options, owned paths, invocation/hash binding, stale receipts and
successful/failed recovery. They never access EditorPrefs or launch an Editor.
