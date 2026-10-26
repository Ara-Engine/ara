# ara-geo

Deterministic offline virtual geometry cooking. No GPU, filesystem, environment,
clock or random input participates in cooking. Install GeoImporter in an asset
worker; never cook on the editor/render thread.

API: cook, Geometry::{encode,decode,cluster}, GeoImporter, SourceMesh, Settings.
Meshlets have at most 64 vertices and 128 triangles. meshoptimizer partitions
adjacent meshlets, simplifies each group with locked borders, and reclusters.
Groups are replacement operations, not independent cluster LODs. A group's
replacement clusters share one error/bounds decision; cumulative errors and
enclosing group spheres preserve monotonic selection through the DAG.
Compaction assigns local IDs in first-encounter index order. Reuse a source-sized
generation-tagged lookup across groups; sparse public inputs keep the ordered-map
fallback. Never change the vertex/index bytes passed to native algorithms.

All clusters quantize on the same asset-wide grid: shared border positions decode
identically. Vertices and local triangles are independently meshopt-compressed
per cluster page. Format/version and importer version must change together when
algorithms or layouts change. Format metadata is validated before use.

Per-crate gates (CARGO_BUILD_JOBS=2): cargo test -p ara-geo,
cargo clippy -p ara-geo --all-targets -- -D warnings,
cargo fmt -p ara-geo -- --check. The million-triangle deterministic cook is an
explicit ignored test: cargo test -p ara-geo --test cooking -- --ignored.
