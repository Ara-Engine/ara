# ara-patch-charts

Purpose: deterministic 25 cm surface-patch charting for one mesh, with persistent patch
identities and material boundaries. Consumed by `ara-render`'s `look::transport`.

Owner: the render-v3 A2 track (council-v3 architecture, section 6.1).

API: `build(Mesh, Settings) -> Charts`; settings default to 0.25 m cells and a
45-degree crease. Scale is signed/nonuniform, applied before clipping. Rigid
placement is renderer-owned. Charts are shared for identical mesh+scale keys.
Cell/material/normal-hemisphere order and edge-connected components are stable;
final FP32 records retain source-triangle barycentric coverage. Never use BLAS
permutation indices as source triangle IDs.

`cache::{Key,prepare,encode,decode}` provides SHA-256 keyed, checksummed binary
side-cars. The caller owns filesystem I/O and atomic publication. Missing/corrupt
caches rebuild; no asset-format migration. Bump VERSION if ordering/math changes.

Test: `cargo test -p ara-patch-charts`; strict clippy on all targets. Outputs and
bench measurements belong under target/ or the explicit render-v3 measure root.
No solver, GPU runtime, parallel reductions or hidden I/O in the library.
