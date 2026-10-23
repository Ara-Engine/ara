# Cell streaming evidence

Measured on 2026-10-23 with Rust 1.99.1, on the shared Windows 11 development
machine (Ryzen 7 7800X3D, 32 GB RAM), with `CARGO_BUILD_JOBS=2` and two
below-normal-priority cell workers. The final optimized code is `f0c57d7`, with
main through `8ddc11b` merged before its local gates.
No renderer, GPU uploads, physics, network transport or gameplay systems run in
this benchmark. These measurements cover world/ECS streaming, not complete game
frame time. The scoped unit/integration tests cover the authoring and cache paths.

Reproduce the optimized run with:

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo bench -p ara-world --bench flythrough
```

The fixture is a 4 km by 4 km world: 256 cells of 250 m, with exactly 1,000,000
entities and ordinary Transform components. It writes 56.01 MiB of checksummed
cooked cell data before measurement. Two serpentine camera passes traverse every
cell at a nominal 75 m/s. Worker scheduling is accelerated with a 100 us yield
outside the measured section. Every cell must fully activate at least once.

Activation gets a 450 us time budget and a 64-entity ceiling. Distance to cell
bounds and 0.5 s velocity prediction drive loading; unload grace is eight host
ticks. Each admitted cell reserves 16 MiB; the configured admission ceiling is
192 MiB. In-flight, active and retiring cells hold reservations until worker
destruction acknowledgement. Admission accounts for decoded source rows,
materialized defaults, HLOD metadata, staging and ECS/mapping overhead.

The main-thread sample includes entity creation/destruction, component insertion,
stable mapping updates, lifecycle selection and retirement scheduling. It excludes
file I/O, decoding, schema validation, worker destruction and gameplay systems.
Busy-frame percentiles exclude zero-entity frames; all-frame percentiles include
them. Final draining is measured too. An empty Local tick baseline measures the
same boundary without streaming. Whole streaming-boundary timings include its
normal fixed-tick bookkeeping.

The benchmark fails unless busy-frame p99 is below 1 ms, at least 1M rows spawn,
all 256 cells fully activate, and final entity count/reservations return to zero.
Working-set/private memory are sampled through GetProcessMemoryInfo every 256
frames. Reservation/retained estimates describe cell ownership; they are not an
OS allocator or GPU memory limit.

The final optimized run passed in 64.52 s, with 102,471 measured frames and 63,334
busy frames. It spawned and despawned 1,996,094 rows; all 256 cells fully activated.
Busy p50/p95/p99 were 0.0671 / 0.1155 / **0.1436 ms**. All-frame p50/p95/p99 were
0.0479 / 0.1000 / 0.1371 ms. Empty/streaming whole-boundary p99 was
0.0002 / 0.1386 ms. The maximum activation sample was **6.5174 ms**.

Peak sampled working set was **112.44 MiB**, private memory 113.53 MiB,
admission reservations 32 MiB, estimated retained cell memory 30.90 MiB, and active
entities 7,814. Final entities and reservations were both zero. BENCH.txt contains
the final raw metric lines.

Immediately before removing cell-key allocation from work selection, the same
optimized fixture passed at busy p99 **0.3844 ms**, maximum 8.6856 ms, with
108.60 MiB sampled working set and 48 MiB peak reservations. That run had 60,184
busy frames and 1,894,794 matching spawn/despawn operations. The final path uses
mutable iteration to borrow the selected key/state directly. These runs share a
busy development machine and different worker-completion timelines; the measured
before/after difference does not isolate allocation cost from scheduler noise.

Local gates after the main merge passed: `cargo check`, 29 focused tests,
`cargo clippy --all-targets -- -D warnings`, and `cargo fmt -- --check`, scoped to
ara-world/ara-sim. Coverage includes canonical round trips, independent-cell and
same-cell entity merges, atomic stale-base-safe moves and undo, cross-cell path
repair, actor files, unloaded-cell queries, cache reads, activation/unload budgets,
grace/layer changes, memory-pressure eviction, HLOD/default admission, pack patch
isolation, stable mappings, snapshots and deterministic streaming replay.

An earlier development-profile run (`--profile dev`, engine opt-level 1,
dependencies opt-level 3) passed with 102,474 frames, 63,892 busy frames and
1,989,499 matching spawn/despawn operations. All 256 cells fully activated.
Busy p50/p95/p99 were 0.2177 / 0.3761 / 0.8460 ms; all-frame p99 was 0.5743 ms.
The empty/streaming boundary p99 was 0.0003 / 0.5956 ms. Peak sampled working set
was 114.82 MiB, private memory 118.03 MiB, admission reservations 48 MiB, and active
entities 7,814. This run preceded the stricter default/HLOD admission accounting.

The development run's maximum activation sample was 21.5549 ms. The scheduler
stops starting entity operations at its deadline; a single operation, allocator
growth or OS preemption is indivisible. Meeting the p99 gate does not establish
a hard worst-case bound or explain those outliers. Full-game GPU/physics/net and
asset-upload budgets need separate integration measurements.

The parent integration track owns host/CLI schema and dispatcher registration:
ara-host currently exposes only single-document cmd.apply through service Cx,
while world moves need its journal/review drain around multi-document kernel
transactions. Renderer extraction consumes HLOD/visibility callbacks; net adapters
consume cell readiness/removal and per-source interest, retaining ownership/private
state filtering. Editor adapters publish replacement text indexes and obey D20
generation/swap policy. The library does not bypass these host responsibilities.
