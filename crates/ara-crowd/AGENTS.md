# ara-crowd

Deterministic parallel crowd avoidance (ORCA) and shared-goal flow fields.
Pure maths: no ECS, navmesh, native code or clock. Depends only on Rayon.
`ara-nav` gathers agents from ECS, calls `Crowd::step`, and writes results back.

## Public API

- `Crowd::new(Params)`, `Crowd::step(&mut [Agent], walls: &[&Walls], dt) -> Stats`:
  one fixed step on the caller's Rayon pool. Agents carry a stable `id` (tie-break),
  position/velocity/preferred velocity, radius/height, speed/acceleration limits,
  avoidance `layers`, a wall-set index (`walls`, or `NO_WALLS`) and `obstacle`.
- `Walls::new(&[Wall], cell)`: immutable navmesh boundary segments with a hashed
  cell index. Build once per navmesh revision off the tick thread. One set per
  agent profile; agents pick theirs by index, so all profiles share one solve.
- `WalkGrid::{new, rasterize}` + `FlowField::build(&grid, goal)`: Dijkstra over an
  8-connected walkable grid (no corner cutting); `direction`/`distance`/`height`
  are O(1) lookups. One field per shared goal; single walkable layer only.

## Determinism contract

- Each agent reads only the previous step's bodies and writes only its own output
  (Jacobi update). Neighbours rank by `(distance², id)`, walls by `(distance², index)`,
  so results are identical for any input order and any worker count (tested).
- Grid buckets are built by a counting sort in input order; bucket pruning only
  skips buckets strictly farther than the K-th neighbour, so it is exact.
- Same binary/platform gives identical bits; cross-platform FP is not promised.
- Duplicate IDs are the caller's error (ara-nav rejects them before stepping).

## Model and gotchas

- ORCA after van den Berg et al. 2011, written for Ara (no third-party code).
  Agent-agent half-planes share responsibility 0.5; `obstacle` agents never move
  and others take full responsibility.
- Walls are hard half-planes ("never approach faster than clearance / horizon"),
  plus crossing clipping after integration. Navmeshes are already radius-eroded,
  so `wall_margin` is small. Walls do not know area filters: avoidance pushes can
  enter forbidden areas; path following steers back.
- Vertical filtering: agents interact only when their height ranges overlap; walls
  apply within one agent height of their Y range (stacked floors stay separate).
- Y velocity follows the preferred velocity's slope scaled by the chosen speed.
- `Params::neighbor_distance` is also the grid cell size; walls are only seen
  within one wall cell, so pick the wall cell >= wall look-ahead distance.
- Flow fields: 4M-cell cap; stacked floors need per-agent navmesh paths instead.

## Testing

Set `CARGO_BUILD_JOBS=2` (shared host), `CARGO_INCREMENTAL=0`.

```
cargo test -p ara-crowd
cargo clippy -p ara-crowd --all-targets -- -D warnings
cargo fmt -p ara-crowd -- --check
cargo bench -p ara-crowd --bench crowd   # 10k four-way crossing at 1/2/4/8 workers
```

Tests: head-on deviation, wall sliding, counterflow arrival without overlap,
bit-identical 1/4/8 workers, input-order independence, corridor walls and pillar,
vertical/layer separation, obstacle discs, flow-field routing, input validation.
