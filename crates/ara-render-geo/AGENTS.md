# ara-render-geo

Virtual geometry GPU extraction over ara-gpu and ara-render::Graph.
API: GeometryView, Instance, View, Outputs, DebugView, PagePool.
All geometry is static opaque single-material; transforms are uniform scale plus
translation. Reject unsupported input explicitly. These are render snapshots,
never authoritative ECS state. Scene preparation/cooking is off the frame thread.

GPU passes: reset, previous-HZB instance cull, projected group-error DAG cut,
cluster cull, SW small-triangle raster, mesh-shader/indirect HW raster, current-HZB
retest, second raster, surface resolve. Visibility atomically packs reversed depth
and a draw/triangle address; resolve exports full instance/triangle U64 identities.
Never choose individual LODs within one replacement group. Streaming publication
requires the complete asset level to be resident; roots stay pinned. Missing fine
levels conservatively fall back to resident coarse geometry, with a diagnostic.

record updates graph states: submit before recording again. Host owns completion
and calls update only after its frame slot completes. GPU counts drive draws;
diagnostic capture alone reads counters/images synchronously. Explicit history
reset is required on resize, camera cuts or geometry replacement.

Gates with CARGO_BUILD_JOBS=2 and Vulkan SDK: cargo test -p ara-render-geo,
cargo clippy -p ara-render-geo --all-targets -- -D warnings,
cargo fmt -p ara-render-geo -- --check. Cook shaders offline with ara-shader-cook.
Read INTEGRATION.md before wiring fidelity/editor. Never edit ara-render-fidelity
from this track; its active owner owns the external surface/shadow input seam.
