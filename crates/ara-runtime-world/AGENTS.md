# ara-runtime-world

World streaming in play: the play snapshot's `.world` (ara-world) streams its cells
into the running simulation around the players, at the session's streaming boundary.
Depends on ara-world, ara-runtime, ara-sim, ara-physics.

## Public API

- `WorldStreaming::from_workspace(workspace, schema)` → `Option<(plugin, Receiver<CellEvent>)>`
  for the snapshot's first world document. Install it **last** among runtime plugins:
  `register` clones the registry into the world workers (rows are prepared against it).
  - `before_tick`: interest from every `PlayerControl` entity's `WorldPose` (and
    `Velocity` for lookahead), in player order; `Streaming::update` at tick + 1.
  - `stream`: `Streaming::activate` inside `Session::step_with_streaming` (64 entities
    per boundary; the arrivals are recorded in the tick record and replay exactly).
  - `mirror`: a networked client's never-stepped mirror streams around its own player
    through `Session::mirror_streaming` at the presented authoritative tick.
  - `is_resident(cell)`, `manifest()`.
- `CellEvent::{Ready, Removed}` (cell ID + `CellKey`), in activation order: consumers
  load and unload per-cell presentation data (baked `AcousticCell` probes via
  `Cell.acoustics`, terrain tiles).
- `SnapshotSource`: `CellSource` cooking cells from the snapshot's in-memory documents
  (`cook_cell`, no HLOD) on the world worker threads.

## Runtime support for streamed entities

ara-runtime adopts entities spawned after build: `physics.adopt` poses new simulated
bodies, `animation.slots` gives new animators their slot, and `Runtime::new` decodes
the models referenced by every snapshot document (decoding never happens in a tick).
Networked authorities stream through `SessionAdapter::stream`; replication carries
streamed entities to clients, whose mirrors stream the same cells locally.

## Gotchas

- Cell scenes are separate documents; the played level must not also contain them.
  Streamed identities (record IDs) must be unique across the level and every cell.
- Navigation agents in streamed cells are not adopted yet (keep NPCs in the level).

## Testing

`cargo test -p ara-runtime-world` (event forwarding). End to end with a 4-cell world,
terrain and a replicated host + client: `cargo test -p ara-player --test valley`.
