# ara-nav

Navigation owns authoring schema declarations, raw-triangle baking, tile caches,
immutable runtime meshes, queries, stateless Detour crowd avoidance and ECS drivers.
Y-up metres, positive-Y triangle winding. Area IDs are 0..15; 255 is non-walkable.

Public entry points: `register`, `register_sim`, `Baker`, `BakeJob`, `NavMesh`,
`NavQuery`, `NavWorld`, `goto`, `why_unreachable`, `DebugPrimitive`.

`Baker` is synchronous OFFLINE/WORKER ONLY. Editors use `BakeJob` with the
`ara-nav-worker` executable: below-normal-priority process, nonblocking `poll`,
cancel on drop. Keep old meshes until an explicitly allowed tick-boundary swap.
Incremental requests must include the whole current geometry/obstacle/link set;
tile hashes determine what actually needs rebaking. Supply dirty bounds for removed
geometry when selecting a subset of streamed tiles; don't discard neighbor halos.

`native.rs` is the sole unsafe boundary. Native meshes become immutable before
sharing; queries own scratch and borrow the mesh. C++ does not retain borrowed
Rust slices. Rust staging buffers are reused; native crowd scratch resets from ECS each step, so no
native agent state survives the tick. Server authority alone steps crowds.

Gameplay truth is `NavAgent` + `NavAgentState`; include both in snapshots. Sort by
`NavAgentState.id`, never entity allocation/iteration order. Fixed dt is configured
once. Off-mesh traversal progress is also in `NavAgentState`. Client worlds only
apply replicated components. `NavPosition` is the transform integration seam.
Authoring edits use existing typed kernel component commands; runtime goto is an
intent, never a document write. No dependencies on unfinished sibling tracks.

Run with `CARGO_BUILD_JOBS=2`:
`cargo check -p ara-nav -p ara-nav-sys`
`cargo test -p ara-nav -p ara-nav-sys`
`cargo clippy -p ara-nav -p ara-nav-sys --all-targets -- -D warnings`
`cargo fmt -p ara-nav -p ara-nav-sys --check`
`cargo bench -p ara-nav --bench navigation` (only when benchmarking is authorized).

Tests cover tile seams, ramps/stairs, stacked bridges, links/areas, cut-outs,
cache corruption, incremental invalidation, worker publication, replay and 500
agents. See README.md for measured results and integration limitations.
