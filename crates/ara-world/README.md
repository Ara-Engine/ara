# Partitioned worlds

`ara-world` owns authoring layout and the host-side cell lifecycle. `ara-sim`
applies prepared spawn/despawn rows at fixed boundaries; authoritative gameplay
remains in registered ECS components.

```text
levels/outpost.world                 manifest: w_grid, l_gameplay, l_night, c_00
levels/cells/0_0/gameplay.scene       ordinary flat Entity document
levels/cells/0_0/art.scene            separate layer/cell partition
levels/actors/e_radio.scene           optional heavily edited actor subtree
.ara/cache/{actions,objects}/...      content-addressed derived data
world/cells/c_00.aracell              cooked, independently chunked pack entry
```

Manifest cell records keep primary scene references in `documents` and standalone
actor references in `actors`. They reference scene documents as
`@levels/cells/0_0/gameplay.scene#sc_gameplay00`; cross-cell entity addresses are
`@levels/actors/e_radio.scene#e_radio`. Every scene/entity ID is stable across
cell membership changes. Canonical scene files merge using the existing
`ara merge`; register the world schema before reading or merging manifests.
Add `*.world merge=ara` alongside the project's scene/prefab git attributes when
the host/CLI loads this crate's schema (the parent integration owns that wiring).
Use `WorldEntityRef` for component fields that reference entities across cells;
its ID-authoritative lookup repairs stale paths through `TextIndex`. It avoids
asset-reference repair, which indexes document assets rather than entity IDs.

The four typed tools are `world.cells.list`, `world.query`,
`world.entity.move_to_cell`, and `world.layer.set`. `WorldTools::catalog` generates
their parameter schemas. `WorldTools::call` takes the authoritative kernel Store
and authenticated transaction metadata. Moves and layer edits produce ordinary
undoable transactions. A subtree that occupies an entire actor file moves by
manifest membership, preserving the readable filename. Inline subtrees move with
cross-file reference repairs in the same transaction. Invalid/stale moves roll back.

Build/publish `TextIndex` on a worker from canonical source snapshots. Queries use
actual world-space entity points, including unloaded and hidden layers; pass a
layer filter when desired. A stale revision is rejected until the adapter publishes
its replacement index. Changing membership does not imply changing transforms.

`cook_cell` validates and flattens scene/prefab rows, orders parents first, and
optionally calls `HlodCooker`. `CellBlob` is checksummed/versioned derived data;
its schema hash prevents incompatible publication. Each `pack_input` is one
logical file, so ara-pack's chunks never span cell boundaries and patches leave
unmodified cells' chunk identities intact.

Create a `WorkerPool` from a `CookedSource` (shipping binary files), `PackSource`
(individual ara-pack entries), or `EditorSource`
(text snapshots hashed into ara-asset's binary cache). Loading, parsing, validation
and destruction run on workers. Editor prefab snapshots use `with_dependencies`.
Use the same cell reservation in the pool and runtime; their constructor checks it.
Call `Streaming::update` with player/camera sources,
explicit host ticks and the latest committed layer state. Distance to cell bounds
and projected velocity determine requests. Memory pressure evicts less useful cells;
grace periods absorb short range oscillations. Hidden layers stay loaded when enabled.

At a fixed tick, call `session.step_with_streaming(|boundary| { ... })` and run
`streaming.activate(boundary, budget, interest)` inside the closure. Bound both time
and entity count. The budget stops starting operations; one ECS entity operation
is indivisible, so OS preemption and component allocation can exceed a deadline.
Recording captures the actual structural arrivals; replay needs neither workers
nor wall-clock decisions. Snapshots preserve streamed mappings. The host must reset
its streaming generation after snapshot restore so lifecycle counters agree.

Drain the old runtime with no sources before dropping/replacing it, so prepared
buffers and cells retire on workers. Renderer/net adapters implement `Interest`.
A cell becomes ready only after all
rows activate. Visibility events and HLOD descriptors feed renderer extraction;
cell readiness/removal delimit net baselines and interest groups. Ownership and
private-field filtering remain the net adapter's responsibility. `relevant_cells`
provides read-only per-connection source interest. Local sessions
link no networking code. Persist authoritative gameplay before unloading, through
the game's registered ECS snapshot/persistence policy.

Host registration is an integration seam: the current ara-host Cx exposes only
single-document `cmd.apply`, so it cannot yet dispatch these multi-document world
transactions. The parent host track must expose the typed dispatcher through its
journal/review drain and register the world schema/CLI merge kind. This crate does
not create a second host store or bypass its authorization/journal.
