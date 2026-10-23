# ara-world

Partitioned canonical world documents, text indexing, cell cooking and bounded
host-side streaming. Depends on the authoring kernel, ara-sim, ara-asset and ara-pack.

## APIs

- `register`: `world` kind (`.world`), `WorldSettings`, `Layer`, `Cell` records.
- `Manifest::read` / `documents`: validated root and cell scene references.
- `TextIndex::build` / `query`: immutable spatial index covering unloaded cells.
- `WorldCommand::{MoveToCell, SetLayer}::apply`: attributed kernel transactions,
  including atomic multi-document moves, undo, stale bases and idempotency.
- `WorldTools::{catalog, call}`: four reflected typed tools. The protocol adapter
  authorizes calls and journals kernel commits. Replace stale indexes on a worker.
- `cook_cell`, `CellBlob::{encode, decode, pack_input}`, `HlodCooker`: offline
  projection, checksummed CBOR derived data, cell-isolated pack files, renderer seam.
- `CellSource`, `CookedSource`, `EditorSource`, `WorkerPool`: worker-only loading.
- `Streaming::{update, activate}`, `Interest`: source/layer interest, bounded
  activation, unload grace, memory admission, renderer/net lifecycle callbacks.

## Invariants

Authoring changes always produce ara-ops transactions; never mutate source files
from streaming. Persist text only with ara-doc's canonical writer. Actor files
retain paths when cell membership changes; entity IDs are unique across a world.
Inline moves relocate whole local subtrees and repair cross-file references in
the same transaction. Reparent an inner entity to a root before moving it.

No I/O, waits, schema traversal or whole-cell destruction in activation. Prepared
rows move into ara-sim only via `Session::step_with_streaming`. Host timing stays
outside deterministic simulation; recording captures arrivals for replay.
Parents activate first; children unload first. Save persistent gameplay state
before unloading (streaming never stores authoritative gameplay in this crate).

Reservations cover in-flight, prepared, resident and retiring cells. Worker
transient buffers are separately limited to one eighth of the cell reservation.
Rows over 16 KiB are rejected; move bulky data to assets. Retire cells on workers;
release reservations only after destruction acknowledgement. A failed cell needs
an explicit retry. Schema/file generation changes require a new source/pool;
retain the last good generation until its D20 application boundary.

`Interest` is a seam, not a transport or security policy. Net adapters must also
apply ownership/private-state filtering. HLOD generation is a cooker callback;
the renderer owns mesh/impostor generation, visibility and transition blending.

## Gates

Set `CARGO_BUILD_JOBS=2` on the shared Windows machine.

```
cargo check -p ara-world -p ara-sim
cargo test -p ara-world -p ara-sim
cargo clippy -p ara-world -p ara-sim --all-targets -- -D warnings
cargo fmt -p ara-world -p ara-sim -- --check
cargo bench -p ara-world --bench flythrough
```

The benchmark generates 1M entities over 16 km² / 256 cells, runs two serpentine
camera passes, includes spawn/despawn churn, and checks busy-frame p99 < 1 ms,
at least 1M activations, and final empty residency/reservations. BENCH.md records
hardware, profile, frame/worker limits, actual measurements and their scope.
