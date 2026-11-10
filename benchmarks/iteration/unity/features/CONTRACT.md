# Matched ECS feature fixtures

Status: implementation contract, not measurements. These are new Ara-owned
fixtures, separate from the unchanged S8 kernels. Each case must have a real
Unity Entities/Jobs/Burst implementation and a Bevy implementation before timing.
No raw-array substitute may be labeled an ECS API comparison.

## Common state and verification

Use persistent ECS components, stable IDs `0..count`, fixed integer seed, and
explicit unsigned 32-bit wrapping arithmetic. Avoid FP differences in this suite.
Set initial `Value = Mix(id ^ 0x6d2b79f5)`; `Mix` is:

```text
x = (x ^ (x >> 16)) * 0x7feb352d
x = (x ^ (x >> 15)) * 0x846ca68b
return x ^ (x >> 16)
```

Every multiply/add wraps modulo 2^32. One update is
`Value = Value * 1664525 + (1013904223 ^ Mix(id + tick))`.
Tick starts at zero and advances through warmup and measured steps together.
An independent scalar reference must agree before either engine is timed.
[reference.py](reference.py) supplies that oracle; it is not a throughput lane.

Export every authoritative field, tag-presence bit and buffer element in stable-ID
order, with a versioned little-endian header and lengths. Compare complete BLAKE3
digests for every engine/case/round, not just sums. Check intermediate dependency
columns and the number of updates too, so skipped jobs cannot pass by leaving the
same final payload. Rendering, setup, export and hashing remain outside timing.

The binary header is `<8sIIII`: magic `ARAECSF1`, version 1, case ID (table order,
1..6), entity count and total steps. Each record is nine little-endian u32 values:
ID, tags, Value, DirtyEpoch, A, B, C, update count, buffer length, then that many
u32 buffer words. Inactive fields are zero. Query tags use three/six low ID bits
for eight/64 archetypes; other cases begin without tags. This format is independent
of either engine's physical component layout.

Construct worlds, retained queries, schedules and scratch buffers once. Include
the specified marking, dispatch, joins and structural playback inside each timed
step. Run 120 warmup + 300 measured steps, two fresh-process rounds reversing
engine/case order; use niceness 10, the controlled Burst wrapper, load/power
capture and the same preference-restoration proof as S8. Thread arguments are
backend-specific: record configured workers, participating caller and observed
runner ceiling, never assume every scheduler has Bevy query semantics.
The implemented one-runner query fixture uses Unity `Schedule + Complete` and
Bevy's direct contiguous loop. Both dispatch costs remain timed, so that row does
not isolate equal scheduler overhead.

## Cases

| Case | Scales | Timed contract | Actual Unity API | Bevy counterpart |
|---|---|---|---|---|
| `query_filter` | 100k/1M; 8/64 archetypes | Static tags determined by ID bits; update only `TagA && !TagB`. Report matched/live entity counts. Retain the query across steps. | `EntityQueryBuilder.WithAll<StableId, TagA>().WithAllRW<Value>().WithNone<TagB>()`, scheduled `IJobEntity` with that query. | Retained `QueryState` with `With<TagA>, Without<TagB>`, contiguous/parallel iteration where supported. |
| `dirty_epoch` | 100k/1M; dirty 0/1/10/100%; clustered/spread | Mark the deterministic dirty set, then update only rows whose explicit `DirtyEpoch == tick + 1`. Include marking and consumer work. Report logical dirty count and candidate visits separately. | Writer system plus consumer `IJobEntity` with `[WithChangeFilter(typeof(DirtyEpoch))]`; the job still checks explicit epoch because the filter is chunk-conservative. | Writer/consumer systems with `Changed<DirtyEpoch>` plus the same explicit epoch check. |
| `archetype_churn` | 100k; 1/10/100% | Alternate adding/removing `TagD` for a fixed ID subset, apply commands at the same boundary, then update only rows with `TagD`. Include recording, playback and query work. | `EntityCommandBuffer.ParallelWriter.AddComponent<TagD>/RemoveComponent<TagD>`, `Playback`, then scheduled query. | `Commands`/command queues with stable playback order, `apply_deferred`, then matching query. |
| `dependency_dag` | 100k/1M; chain/fork-join | Four stages over component columns, complete only at the final dependency. Include every stage and join. | Burst `IJobEntity.ScheduleParallel`, explicit `JobHandle` edges, `JobHandle.CombineDependencies`, final `Complete`. | Retained ordered system schedule with independent component access; chain/fork-join edges declared explicitly. |
| `buffer_fixed` | 10k/100k; lengths 0/4/16/64 | For every element, apply the same wrapping update using entity ID and element index. No resizing in measured steps. | `IBufferElementData`, `[InternalBufferCapacity(4)]`, `DynamicBuffer<Word>` in a scheduled job. | Component-owned `SmallVec<[u32; 4]>`, preserving all gameplay state in ECS. |
| `buffer_resize` | 10k/100k; 10%/100% resized | Alternate length 4/64. Compare capacity-reuse and explicit-trim modes separately; update all live elements after resize. Include growth/copy/trim work. | `DynamicBuffer.ResizeUninitialized`, initialize newly visible elements, and optional `TrimExcess`. | `SmallVec::resize` plus initialization and optional `shrink_to_fit`; verify inline/spilled behavior before comparison. |

Do not call Unity's chunk change filter equivalent to a per-entity dirty bit.
For `dirty_epoch`, the explicit epoch check makes final update semantics equal;
candidate visits deliberately expose different tracking granularity. Measure visits
in a separate instrumented correctness run, not with a contended per-entity atomic
inside the throughput loop. Mark only selected rows; obtaining mutable access to
every chunk would otherwise make the dirty-fraction sweep meaningless.

Dirty selection is either `id % 100 < percent` (spread) or a rotating interval of
`count * percent / 100` IDs starting at `(tick * 997) % count` (clustered), wrapping
at the end. Percent zero still
executes the same scheduler boundary with no logical updates. Epoch values are
part of exported state. Warmup must exercise the same dirty distribution.

For `archetype_churn`, begin with no `TagD`; add on even ticks and remove on odd
ticks. Update `Value` after playback only where the tag is present. This exposes
incorrect playback ordering even when the final tick has no tags. Entity count
and stable identity remain unchanged; S8 W4 separately covers destroy/spawn.
Use Bevy's default table storage for this tag; a sparse-storage variant is a
separate explicitly labeled experiment, not a silent avoidance of table movement.

The fork-join contract is:

```text
A = Mix(Value + tick)
B = A * 3 + id             (depends on A)
C = A ^ (id * 5 + tick)    (depends on A, independent of B)
Value = B + C              (depends on B and C)
```

The chain variant executes these same four stages in order. Export A/B/C/Value.
Do not combine the arithmetic into one kernel or complete after each independent
job: that would erase the scheduler/dependency question being measured.

For buffers, the logical contract matches while physical storage may differ.
Record element type, component stride/alignment, inline capacity, live length,
capacity and spilled/inline state. Resizing down does not imply freeing capacity.
Unity's `TrimExcess` can move short buffers back into chunk storage; verify the
selected Rust operation's corresponding behavior rather than assuming it. Native
allocation counts require explicit allocator instrumentation; managed GC counters
alone do not describe Unity's native-buffer allocations.

Buffer word ID is `entityId * 64 + elementIndex`; initial/newly visible words are
`Mix(wordId ^ 0x6d2b79f5)`. Apply the common update using word ID, then add the
wrapping sum of all updated live words into the entity's Value. This retains the
effects of grown-tail work even after a later shrink. Fixed buffers and every
resize-case entity increment the update counter once per step; zero-length fixed
buffers still execute that bookkeeping. Resize cases start at length four and
grow selected IDs on even ticks, shrinking them on odd ticks.

## Installed-source checks

The API names above were checked read-only in installed Unity 6000.4.5f1's
`BuiltInPackages/com.unity.entities/Unity.Entities` sources:

- `Iterators/EntityQueryBuilder.cs`: `WithAll`, `WithAllRW`, `WithNone`.
- `IJobEntity.cs`: `WithChangeFilterAttribute` and generated scheduling contract.
- `EntityCommandBuffer.cs`: typed add/remove and parallel command writer.
- `Iterators/DynamicBuffer.cs`: `ResizeUninitialized`, `Capacity`, `TrimExcess`.
- `SystemState.cs`: change-filter version is assigned from the system's prior run.

Use the companion primary-document research to pin public documentation links
before implementation review. These inspected sources establish availability,
not that the new fixtures already compile or have measurements.

## Delivery order and gates

1. Finish untouched S8 W2 20k/50k and W4 100k with complete state equivalence.
2. Implement integer state/reference/export and `query_filter` first; smoke both
   engines, verify all state and the controlled wrapper, then measure.
3. Add dirty epochs and dependency DAG, checking system change versions and
   observed concurrency independently of throughput timings.
4. Add archetype churn and buffers, measuring structural/allocation work in the
   stated window. Keep losses and tail regressions in the result.

Implement new fixture files here and copy them only into the owned scratch
project. Any Rust fixture crate must be standalone, reuse installed dependencies
and avoid changing the engine's production APIs just to win a benchmark.
