# Ara navigation

Production navigation boundary for A11, using Recast/Detour 1.6.0 at
`b4554541b658630816dba41466eb1cefb624519e`. The companion `ara-nav-sys`
vendors unmodified zlib sources and owns a small C shim built with `cc`.
There is no CMake, Python, network fetch, bindgen or prebuilt native library in
the build. See [the dependency decision and license](../ara-nav-sys/VENDOR.md).

Recast supplies the geometry voxelizer, clearance/slope filtering, watershed
regions, contours and detail meshes; Detour supplies tile stitching, A*, funnel
paths, surface rays and nearest points; DetourCrowd supplies local avoidance.
Landmass would leave Ara owning the geometry baker, the largest quality and
maintenance risk. This follows the locked Recast decision rather than reopening it.

## Baking and publication

`Geometry` accepts Y-up world-space indexed collision triangles. Each triangle has
an area ID 0..15, or 255 for solid non-walkable geometry. Winding determines the
walkable side. Agent radius, height, climb and slope are in `AgentProfile`; submit
one request per profile. Keep a stable `BakeSettings.origin` and vertical range.

`BakeInput.tiles` is the complete desired loaded set. `tiles_in` selects a finite
region, including negative coordinates. Omitted tiles stream out, empty tiles are
tombstones, and each tile sees a conservative radius/voxel halo. Edits on a seam
invalidate both neighbors. Inputs are canonically sorted, independently of producer
triangle order or vertex indexing. Temporary boxes are radius/height-expanded
cut-outs. Removing a box restores the original tile's cache identity.

`Baker::bake` is synchronous and intended **only for workers, offline tools and
tests**. Editor integration uses `BakeJob::spawn(worker_path, WorkerRequest)`:

1. Serialization and process launch happen off the tick thread.
2. `ara-nav-worker` runs below normal priority (Windows process creation flags;
   `nice -n 10` on Unix), owns Recast allocations, and writes a binary result.
3. The coordinator validates and assembles a complete immutable `NavMesh`.
4. `BakeJob::poll` never waits. Pass its prepared mesh to `NavWorld::stage`.
5. `tick` swaps only when `allow_mesh_swap` permits it. Set this false during Play
   unless the human allowed changes. A failed/cancelled job preserves the live mesh.

Dropping a job requests cancellation; killing/reaping runs off-thread. Worker files
are private temporary artifacts, removed after completion. Each input/output is
bounded to 512 MiB. Cache entries contain BLAKE3 integrity checks, are atomically
published and get rebuilt on corruption. Keys include canonical tile inputs,
settings/profile, native revision/format, target OS/architecture and endian.
Previous artifacts reuse unchanged tile buffers without rebaking. Native bytes
are validated again before Detour sees them; persisted meshes are not portable
between target formats. Keep the artifact content hash in replay manifests.

## Runtime and integration

- `NavMesh::query` creates isolated reusable scratch; `NavQuery` offers `path`,
  `nearest`, `raycast`, `visible` and polygon-area lookup. Paths explicitly report
  partial/capacity results and their nearest reachable endpoint. `visible` checks
  the ending surface height so overlapping bridge floors cannot be shortcut.
- `NavMesh::paths` uses ordered Rayon batches without locking the mesh. A host may
  install its own bounded Rayon pool; the benchmark uses two threads.
- `register(&mut SchemaDb)` adds `NavAgent`, `NavMeshSurface`, `NavLink`, `NavArea`.
  Kernel typed component commands remain the only document mutation path. Geometry
  producers assign `NavArea.area` IDs, and query consumers copy its default cost
  into `QueryFilter.costs`; area costs are finite and at least one for admissible A*.
- `register_sim(&mut World, &mut Schedule, NavConfig)` integrates directly with
  `bevy_ecs` 0.20. Order `NavigationTick` after world-transform synchronization and
  before physics extraction. Registration is idempotence-checked.
- Synchronize physics/world transforms through `NavPosition`. Snapshot/restore
  `NavAgentState` and `NavPosition` together with authored `NavAgent` parameters.
  Host snapshot registration, collision extraction and character-controller
  application belong to the parallel `ara-sim`/`ara-physics` tracks.
- `goto(world, actor, GotoTarget::Entity(target))` or `GotoTarget::Point(point)` is
  a server-authority runtime intent. Entity targets resolve once when submitted.
  Observe `NavStatus` for headless playtest assertions.
- Crowd stepping sorts stable simulation IDs before all native work. Positions,
  velocities, target, filter, path progress and off-mesh interpolation all live in
  ECS components. Native avoidance scratch is disposable each tick, so snapshot
  restore does not depend on hidden C++ state. Clients never run authoritative AI.
  Idle/unreachable agents with valid positions still participate in avoidance.
- `why_unreachable` returns endpoint/corridor evidence, nearest reachable point,
  blocking area, disconnected islands, capacity exhaustion, or a disabled-link
  candidate. Its common `ara-diag` payload uses nav-local `ARA7100`; the kernel
  owner should reserve that range when integrating (this track cannot edit it).
- `debug_stream` emits typed triangles, links and path segments. `DebugPrimitive`
  also defines agent footprints/velocities for host ECS extraction. There is no
  renderer dependency or renderer-specific draw call.

Native capacities are explicit: at most 8192 loaded tiles, 16 semantic areas,
16 distinct avoidance masks in one profile crowd and 32768 query corners.
Detour's 32-bit reference budget further limits polygons per tile as tile count
grows; rejected capacity returns `NavError`. Detour links attach within the
owning tile or neighboring tiles; choose tile width for the authored link span.
Different profile meshes have separate avoidance solves. Applications needing
interactions across profiles should use their common physical character layer.

## Verification and measurements

Windows 11, Ryzen 7 7800X3D, Rust 1.99.1/MSVC, release profile, strict floating
point, shared host with other production tracks. These are local workload numbers,
not a comparison against other engines. `CARGO_BUILD_JOBS=2` was used throughout.

Final benchmark sample (`cargo bench -p ara-nav --bench navigation`):

- Dense 64 x 64 metre floor, **32768 triangles / 100 tiles**: **56.68 ms** bake.
- Unchanged request: **100 reused / 12.82 ms** input validation and hashing.
- One temporary box: **4 rebuilt, 96 reused / 13.16 ms**.
- 10000 paths: **155145/s** sequential, **237327/s** with two query threads.
- 500 moving agents, 120 steady ticks: **3.98 ms p50 / 5.26 ms p95**.

The initial implementation measured 78.69 ms bake, 45.27 ms unchanged, 94818 paths/s
sequential and 19554/s batched. Spatial indexing removed full-world scans per tile;
reusable native query buffers and bounded batches removed scratch-allocation churn.
Additional floor-visibility checks and turn anticipation fixed shared-corner stalls.

Sixteen focused tests exercise procedural rooms, ramps, stairs, an overlapping
bridge, a linked gap, costs/area exclusions, seams, geometry edits, cut-out removal,
streaming tombstones, corrupt cache/native data, schema defaults, process failure,
publication policy, authority, 500-agent replay, snapshot restore and counterflow.
Replay is bit-exact across reversed ECS insertion order. Separate process runs of
the counterflow capture produced identical canonical JSON hashes. Cross-platform
bit equality is **not yet verified**; server authority avoids requiring clients
to reproduce crowd simulation.

`cargo run -p ara-nav --example inspect -- target/nav-inspect.json` exports the
actual typed debug mesh, a linked path and 40 counterflow trajectories over 1800
fixed ticks. All 40 reach their goals. This is a headless navigation check; engine
renderer/character-controller integration requires those tracks to land.

Local gates: `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`
and `cargo fmt --check`, all scoped to `-p ara-nav -p ara-nav-sys`. The native crate
is also rebuilt and tested with a minimal Windows PATH containing only Rust and
Windows system directories, with `cmake` confirmed unresolvable.

## Navigation v2 (large worlds, dynamic world, crowds)

The default crowd solver is now `CrowdSolver::Orca` (`ara-crowd`): deterministic
parallel ORCA against navmesh boundary walls, one solve for every profile, with
budgeted replans and pure-Rust path following over ECS table slices. DetourCrowd
remains available as `CrowdSolver::Detour`. Typed links (walk, jump, ladder,
door, elevator, teleport) have runtime gates (`NavLinkState`) enforced by a
virtual Detour filter; `NavObstacle` carves or relabels tiles with a fixed
publication latency, or acts as an avoidance disc. Worlds stream per cell
(`bake_cell`, `NavStreamer`) and long routes run on an always-resident cluster
graph (`ClusterGraph`, ALT landmarks). Measurements, methods and caveats are in
[RESULTS.md](RESULTS.md); the gameplay AI built on top lives in `ara-ai`.
