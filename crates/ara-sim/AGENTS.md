# ara-sim

Scene projection, fixed simulation, Local sessions, play/test control and replay.
Depends on the authoring kernel and Bevy ECS/tasks 0.20, never ara-proto or netcode.

## Public API

- `SimRegistry::new(schema)`: schema-driven dynamic layouts plus typed Transform.
  Add typed `Schema + Component + Serialize + DeserializeOwned` bridges with
  `register_component`, infrastructure state with `register_resource`, and typed
  input/async-result handlers with `register_command`.
- `SimCommand` + `register_sim_command` / `submit_typed` move typed command values
  straight to Local execution with no serialization. `CommandContext` identifies
  owner, sequence and target; typed validation is explicit. Snapshot/recording opt
  into encoding, and replay invokes the identical feature implementation.
- `EditWorld::new` / `from_workspace`: documents/prefabs -> entities, `Identity`,
  actual Bevy `ChildOf`, and `SceneMetadata` (name/order). `entity` maps addresses;
  `children` sorts by order then stable ID. Do not assume Bevy Children insertion
  order is authoring order. `apply_transaction` / `apply_ops` mirror committed ops;
  unchanged entities/components survive. Prefab resolution/validation currently
  scans the document before applying only changed ECS rows; there is no world reload.
- `DynamicLayout`: native scalar/vector fields inline in a descriptor-backed table
  element, with an owned header for variable/nested schema values. `fields` reports
  offsets. Private encode/decode/drop routines are the only unsafe projection code.
- `Session::local(edit, config, seed, actions)`: owns one world and schedule.
  `add_system` orders systems by registration; deferred structural writes flush at
  tick end, with automatic between-system flushes disabled.
  Use `SingleThreadedExecutor::new()`: Bevy 0.20's derived `Default` leaves final
  command playback disabled, unlike `new`. A boundary-visibility test guards this.
  `inject`, `submit`, `patch` queue action data, typed commands/results and committed
  administrative ops. All are applied before tick systems. Commands are ordered by
  player/sequence and retransmits deduplicated. A handler error faults the session;
  restore a checkpoint before proceeding. Handlers must validate before writing.
- `SessionAdapter`: optional ara-net seam for Listen/Dedicated. The adapter owns
  authentication, ownership checks, limits, codecs and connection state. Local has
  no adapter, replica, transport, per-connection allocation or network serialization.
- `SimRegistry::prepare_stream_entity` validates cell rows on workers.
  `Session::step_with_streaming` supplies an exclusive `StreamingBoundary` before
  tick systems. Spawn parents first; despawn children first (no cascading unload).
  Streamed IDs share the stable mapping; snapshots preserve it, and hash/history
  sampling records actual arrivals in `TickRecord::streaming` for replay.
  Host timing, range planning, memory limits and I/O belong in ara-world.
- `SimulationFeature`: typed action conversion, complete restorable component state,
  one fixed step and a presentation interpolation function. Optional ara-net binds
  this contract to native input/prediction without leaking backend types to gameplay.
  `PredictionKey` identifies effects by player/tick/sequence for replay-safe cues.
- `diagnostic_state` / `replay_tick`: opt-in trusted replay tooling. The former
  extracts registered ECS/context state; the latter re-applies a recorded boundary.
  Never route this administrative op replay API from public game clients.
- `SimContext`: fixed tick/dt, active inputs, stable per-system/entity RNG streams,
  deterministic runtime identity allocator. Gameplay lives in components; resources
  are service/infrastructure state. No global current player or camera.
- `FixedTick`: default 60 Hz, bounded catch-up, interpolation alpha and timescale.
  `Clock` is injected at the host boundary; systems see fixed context time only.
- `PlayControl`: start/stop/pause/resume/timescale, step, watch, until, snapshot/
  restore and record/replay. StepInput offsets start at the next boundary; holds
  and future queued injections survive early stopping. `until` is tick-bounded.
  Stop preserves the initialized world; use a new session/checkpoint to restart.
- `Snapshot`: complete checkpoint of registered components/resources, lifecycle,
  hierarchy, RNG, active/queued inputs, queued commands/ops, dedup state, source
  documents and host accumulator. Restore keeps the same World so query caches
  remain valid. Recreated entities require stable-ID lookup; raw Entity handles
  are not save references. Host clocks are rebased on restore/pause. `to_bytes` /
  `from_bytes` use CBOR and preserve finite IEEE float bits, including signed zero.
  Runtime fields and source mirrors retain explicit defaults, so queued op `prev`
  preconditions survive restore. Scene identity still uses the canonical doc writer.
- `StateProvider` + `Session::add_provider`: subsystem-owned state (native physics,
  nav agents, animators). Owned types are exempt from the registered-state check;
  `capture` → `Snapshot` (+ state hash), `restore` rebuilds after registered state.
  `Session::checkpoint`/`PlayControl::checkpoint` add exact in-process `TrustedState`
  (never serialized); `record` uses it. `add_parallel_systems`: one ordering slot,
  conflicting access rejected before the schedule changes (multi-threaded executor).
  `EditWorld::workspace()` exposes the mirrored documents for live-patch ops.
  `StateProvider::owned_components` exempts runtime-registered IDs without a Rust
  type (C# module columns and resources).
- External components: `SimRegistry::register_external(name)` hands a schema
  component's storage to another runtime (C# via `ara-runtime-script`). Documents
  still validate against its type; the projection queues complete values in the
  `ExternalWrites` resource for the owner to apply in the same tick, and the owner's
  provider captures it. `EditWorld::from_workspace_in(world, ..)` projects into a
  prepared world (one already hosting that runtime). `SimContext::seed()`.
- `Replay`: scene/dependency hash + seed + build/schema hash + initial snapshot +
  applied input/command/op log. Verification checks every tick hash. The build hash
  supplied by the host must cover native/gameplay modules. No hashes or history
  serialization happen in ordinary Local ticks until explicitly enabled.
  Replay has the same binary transport helpers. State hashes sort keys before CBOR
  encoding and preserve signed zero instead of applying authoring default omission.
- `initialize_compute_pool`: configure workers before sessions; an existing host
  pool is reused. Local initializes a default pool if the host has not done so.
  Systems can use Bevy contiguous parallel queries for safe slice/chunk kernels.

## Determinism and state contract

Same binary/modules, platform, schema, system order, seed and tick input/command
stream give identical state hashes. Cross-platform bit-exact FP is not promised.
Mutable future-affecting state must be in registered components or explicit service
resources/context. No mutable captures, Local, static state, wall time or external
RNG in systems. Added/Changed/Removed filters and Bevy allocator order are not
authoritative game state: use explicit tick/state components and stable references.
Accumulate reductions in a defined order; disjoint per-entity work may run parallel.
Snapshots/hash sampling reject unregistered state and entities without Identity.
Bevy's default query filters and empty internal schedule registry are immutable
engine setup, not gameplay resources. Session-owned schedules are the only tick
schedules; external callbacks/service handles must not carry simulation state.
Serde field names must match schema names; schema defaults are materialized before
typed decoding. Skipped authoritative fields are not supported. f32 overflow and
non-finite runtime values are rejected. Each world has its own dynamic component IDs.

## Gates

Set `CARGO_BUILD_JOBS=4`. Run:

```
cargo test -p ara-sim -p ara-input -p ara-components
cargo clippy -p ara-sim -p ara-input -p ara-components --all-targets -- -D warnings
cargo fmt -p ara-sim -p ara-input -p ara-components -- --check
```

Focused tests cover projection/ops/prefabs, injection, explicit system order,
world isolation, complete registered-state restore, recording/replay and clock bounds.
See `BENCH.md` and `benches/run.ps1` for the matched S8 matrix and load evidence.
Benchmarks include snapshot/grid rebuild and structural playback inside timing;
no scratch allocation in W2 ticks. W4 deliberately retains S8's command path.

## Integration boundaries

`PersistenceSelection` + `Session::persistent_state/restore_persistent` provide
ara-save's opt-in component/resource bridge. They preserve tick/RNG/identity cursor,
exclude input holds/pending work/presentation, and prevalidate schemas/typed codecs.
Capture/apply at host-approved tick boundaries; the host enforces server authority
and resynchronizes network baselines after load. Full Snapshot semantics are unchanged.

This crate is the engine library side of play control. A6's child-process player
pool/shared-memory transport, protocol authorization and ara_play/ara_test handlers
belong to the process/protocol integration. No networking stack is linked here;
Listen/Dedicated need the optional M2 adapter. Physics/C# modules must register
their complete state/codecs (or a `StateProvider`) before relying on snapshots;
persistence (`persistent_state`) does not include provider state yet. Binary checkpoint
shared-memory/process transport, columnar cooking and incremental prefab dependency
invalidation are follow-on work.
