# ara-nav

Navigation owns authoring schema declarations, raw-triangle baking, tile caches,
immutable runtime meshes, queries, crowds (ORCA via `ara-crowd`, or DetourCrowd),
typed off-mesh links with runtime gates, dynamic obstacles, agent types,
streaming cells and hierarchical cluster routes. Y-up metres, positive-Y
triangle winding. Area IDs are 0..15; 255 is non-walkable.

## Public API

- Bake: `Baker`, `BakeJob` (worker process), `BakeInput`/`BakeSettings`/`Geometry`
  (+ `append_heightfield` for `ara-terrain` tiles, holes included), `Obstacle`
  (`area` 0..15 relabels, `Obstacle::CARVE` cuts), `OffMeshLink` (kind/duration/
  arc are runtime-only and excluded from tile keys).
- Mesh/query: `NavMesh` (`query`, `paths`, `paths_blocked`, `walls`, `links`),
  `NavQuery` (`set_blocked_links`, path/nearest/raycast/visible).
- Runtime: `register_sim`, `tick`, `NavWorld` (`stage`, `set_bake_source`,
  `rebake_checkpoint`/`restore_rebakes`, `set_cluster_graph`), `NavConfig`
  (`crowd: CrowdSolver::{Orca, Detour}`, `replan_budget`, `link_wait_ticks`,
  `stall_ticks`, `rebake_latency_ticks`), `goto`.
- Components: `NavAgent`, `NavAgentState` (+ `desired`, `issue: NavIssue`,
  `route`, `waiting`, `speed_limit`, `flow`), `NavPosition`, `NavLink` (`kind:
  LinkKind`), `NavLinkState` (gate), `NavObstacle` (`ObstacleMode::{Carve, Area,
  Avoid}`), `NavAgentType` (profile dimensions + per-area cost multipliers).
- Large worlds: `CellGrid`, `bake_cell`/`CellSource`/`CellBake`, `merge_cells`,
  `NavStreamer` (request/poll/wait), `ClusterSummary`, `ClusterGraph::route`
  (ALT landmarks), `Route`.
- Diagnostics: `why_unreachable[_blocked]` (`ReachabilityReason::ClosedLink`),
  `DebugPrimitive`, ARA7100.

## Tick (ORCA, default)

Order: publish staged meshes -> due obstacle rebakes -> link gates/agent types ->
replans (budgeted, agents without a corridor first, stable-ID order; idle agents
are projected once per mesh revision) -> parallel path following over ECS table
slices (pure Rust, no mesh queries) -> one ORCA solve for every profile (each
agent uses its profile's walls) -> scatter. Results are independent of ECS order
and worker count. Duplicate IDs abort the tick with `duplicateAgentId`.

- Corners advance when the agent is on the free side of the outgoing segment's
  line (funnel test); collinear tile/area crossings are skipped at once.
- Mesh swaps (streaming, rebakes) keep each agent's world-space corridor until
  its budgeted replan replaces it: nothing teleports or stops.
- Goals outside the resident mesh: a partial corridor triggers a cluster route
  (`NavIssue::OutsideResident`); legs target portals, partial legs are walked to
  the resident boundary and replanned after the next swap.
- Links: kinds walk/jump (arc)/ladder/door/elevator/teleport. Closed waitable
  links (door/elevator) are planned through; agents wait (`NavLinkState.waiting`
  counts them) and replan around after `link_wait_ticks`. Locked or closed
  non-waitable links are blocked in every query (virtual Detour filter).
- Obstacles: carve/area changes rebake on a worker thread and publish exactly
  `rebake_latency_ticks` later (joined if late) - deterministic. Until then, and
  always for `Avoid`, they are fixed ORCA discs (agents sidestep them).
- Agent types multiply their area costs onto each agent's own filter.
- Flow mode: `NavWorld::set_flow_field(id, field)` (build with
  `NavMesh::walk_grid` + `ara_crowd::FlowField::build` off the tick thread) and
  `NavAgentState::follow_flow(id, goal)`: massed groups steer by one shared field
  and never plan corridors (single walkable layer; ORCA solver only).

## Rules and gotchas

`Baker` is synchronous OFFLINE/WORKER ONLY. Editors use `BakeJob`; streaming
assembly and obstacle rebakes run on threads and publish at tick boundaries.
Snapshot `NavAgentState` + `NavPosition` (+ `NavWorld::rebake_checkpoint` for
pending rebakes). Hosts must record mesh publication ticks of streaming swaps
for replay. Per-cell bakes need halo geometry (>= tile halo) around the cell;
then cell tiles equal whole-world tiles bit for bit. Cluster costs use base area
costs; resident legs use the agent's filter. ORCA walls ignore area filters.
`native.rs` is the sole unsafe boundary; C++ keeps no borrowed Rust slices.
Server authority alone steps crowds; clients apply replicated components.

## Testing

Run with `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`:
`cargo test -p ara-nav -p ara-nav-sys`
`cargo clippy -p ara-nav -p ara-nav-sys --all-targets -- -D warnings`
`cargo fmt -p ara-nav -p ara-nav-sys -- --check`
`cargo bench -p ara-nav --bench navigation` / `--bench scale` (when authorized).

Tests: tile seams, ramps/stairs, bridges, links/areas, cut-outs, caches, worker,
replay, 500 agents, counterflow, doors (wait/open/timeout), locked links and
`ClosedLink`, jump arcs, fixed-latency carving, avoid-only discs, agent-type
costs, Detour solver, cell bakes == whole-world tiles, hierarchical route vs
exact path, streamed agent across resident swaps, heightfield holes.
See README.md and RESULTS.md for measurements.
