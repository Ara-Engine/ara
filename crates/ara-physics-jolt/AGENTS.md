# ara-physics-jolt

Safe implementation of `ara-physics::PhysicsBackend` over the owned Jolt C ABI.
Gameplay/editor/netcode import ara-physics types; only this crate imports ara-jolt-sys.

## Public API

- `JoltBackend` owns one isolated native world; `JoltWorld` aliases PhysicsWorld<JoltBackend>.
- `JoltSnapshot` has private construction. `byte_len`/`as_bytes` are diagnostic views of
  trusted, version-specific native data. It is not a portable network or asset encoding.
- Default `cross-platform-deterministic` propagates to the native crate. Disable default
  features for a fast local-only build. Both modes retain deterministic update ordering.
- Runtime `deterministic_simulation` is independently tunable; turning it off opts
  out of that ordering guarantee, irrespective of compiler floating-point mode.
- `shared_job_pool` borrows the host's initialized Bevy ComputeTaskPool. Initialize
  it through ara-sim first to choose its size. Dedicated workers remain the baseline;
  compare both on the intended contention/workload before selecting the shared path.

## Implementation

- Every unsafe call needs a SAFETY comment. Do not add Rust callbacks to native jobs.
- `&mut self` gates all native mutation and scratch use. Shared methods only read native
  state with call-local collectors. The Send/Sync proof is documented next to the impls.
- Validate handles/configurations and surface native errors as PhysicsError (ARA7001).
- Contacts are sorted by body/subshape/phase, then decoded into solid/trigger effects.
  The contact cache is saved with snapshots so trigger removals replay correctly.
  Virtual-character effects are derived from restorable active contacts (Jolt does not
  save callback bookkeeping); character/trigger and character/character effects replay too.
- `event_capacity=0` installs no native listener and skips decoding entirely. Game
  default opts out; subscribe by choosing a nonzero bounded capacity before creation.
- Shape debug data is copied once during cooking; primitive shapes are cached by exact
  float bits. Snapshots validate configuration and construction fingerprints before restore.
- Cross-world handle mixing is a caller error: handles are scoped to the creating world.
  Native sequence validation rejects stale bodies; character/vehicle/joint IDs are not reused.
- Character IDs are explicit per-world values, avoiding Jolt's process-global ID counter.
- Raw mesh/hull cooking and asset-reference resolution live outside the tick loop (D20).

## Tests and benchmarks

Set `CARGO_BUILD_JOBS=2`; on the shared host also use `RUST_TEST_THREADS=2`.

- `cargo test -p ara-jolt-sys -p ara-physics -p ara-physics-jolt`.
- Character tests port all seven S6 lanes, plus crouch/clearance, jump, push and stance restore.
- Physics tests cover the 10x10 pyramid, 20-box tower, defaults-driven car/braking, eight
  shapes, six joint types, filtered single/batch queries and contact/trigger replay.
- Determinism tests compare each tick in-process, separate processes, at 1/4/8 threads,
  after restoring into the same/fresh world, plus a one-float-bit negative control.
- ECS tests cover stable identity order, lifecycle/config changes, typed impulses,
  character/vehicle/joint writeback, rollback and sticky extraction errors.
- `cargo clippy -p ara-jolt-sys -p ara-physics -p ara-physics-jolt --all-targets -- -D warnings`.
- `cargo fmt -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -- --check`.
- `cargo bench -p ara-physics-jolt --bench scale`: exact S6 5k/20k bowl, 600 ticks,
  1/4/8 threads, 15 velocity iterations, contact collection disabled. Records setup,
  p50/p95 step/extraction, escaped bodies, snapshot bytes, save/restore in canonical JSON.
  `ARA_PHYSICS_BENCH_THREADS=4,8` and `ARA_PHYSICS_BENCH_STEPS=300` narrow a run.
  Add `--no-default-features` for fast-mode comparison. Host contention makes small
  differences noisy; never present a different solver/thread/workload as an equal A/B.
- `benches/run.ps1` builds deterministic, fast and instrumented executables before
  running them sequentially. `performance` alternates Ara/direct C++ every tick,
  records host load, verifies final states and measures native scratch fallback.
- `cargo test -p ara-physics-jolt --features bench --test performance_contract`
  additionally compares the spatial character tree with the original Jolt scan
  through crowd motion, crouching, removal and snapshot restore.

## Presets

| Constructor | Solver | Arena | Effects | Intended trade-off |
|---|---|---|---|---|
| `game_default()` | 15 velocity / 2 position | 256 MiB | opt-in | Retains the S6 tower; avoids the large fixture's heap fallback. |
| `stress()` | 8 / 2 | 256 MiB | opt-in | Throughput experiment; the slender S6 tower is not guaranteed. |
| `deterministic_network()` | 15 / 2 | 256 MiB | bounded, enabled | Replay ordering; also retain the deterministic build feature. |

All retain 2 cm speculative contacts, 0.5 s / 0.03 m/s sleeping thresholds, the
large-island splitter, and per-body opt-in CCD. Smaller worlds can reduce arena
and contact capacity together after measuring fallback; 256 MiB is reserved per
world. Neither named preset nor the build flag proves cross-platform hash equality.

## Integration limits

See ara-physics/AGENTS.md for ara-sim's native snapshot-provider hook, renderer debug
stream and the raw-geometry asset seam. Native rollback intentionally rejects structural
mismatches; lifecycle reconstruction/replay is the simulation host's responsibility.
Cross-platform bit equality has not been verified on the Mac/Linux/mobile machines.
Tracked vehicles and 2D are explicitly unsupported capabilities; wheeled vehicles ship now.
