# ara-bvh

Purpose: CPU builder for compressed-wide BVHs (CWBVH) used by `ara-render`'s `look::trace`
(render v3 `trace()`, council-v3 D1). Triangle meshes in, flat GPU-ready words out. No GPU,
no file I/O (side-car bytes go through the caller). Owner: the render-v3 A1 track.

A Rust port of tinybvh (MIT, commit c9be3103; notices in each ported file and in
`THIRD_PARTY_NOTICES.md`). Islam did not approve vendoring C++, so nothing is vendored.

## Public API

- `Mesh { positions, indices, submeshes }`: one asset. Triangle `i` (indices `3i..3i+3`) is
  primitive id `i` forever; `submeshes` lists ascending first triangles (empty: one).
  `Mesh::hash()` is the SHA-256 content key (`MeshHash`); `validate()` checks indices.
- `Blas::build(&mesh)`: SBVH (binned SAH + spatial splits, exact sweeps up to 512 refs,
  1.5x reference budget) -> SAH-optimal 8-wide collapse (Ylitie 2017) -> CWBVH encode.
  `Blas { hash, nodes, triangles, bounds, submeshes, stats }`; `nodes` are 5 x `[u32; 4]`
  per node (80 B), `triangles` 4 x `[u32; 4]` per reference (64 B). Layout: `cwbvh.rs` docs.
  `BlasStats::max_stack <= TRACE_STACK` always.
- `Blas::intersect(&Ray, RayFlags)` / `intersect_filtered(.., accept)`: CPU traversal that
  mirrors `shaders/trace.slangh` operation for operation (same edge slack, same flags).
  `traverse::brute_force` is the f64 reference; `moller_trumbore` the f64 triangle test.
- `RayFlags`: `ACCEPT_FIRST_HIT`, `CULL_BACK_FACES`, `CULL_FRONT_FACES`, `FORCE_OPAQUE`
  (bits match `TRACE_*` in the shader).
- `tlas::Tlas::build(&[Aabb])`: threaded (stackless) binary TLAS, larger child first;
  `refit` updates boxes in place (false on structural change), `needs_rebuild` after refits
  degrade the SAH cost past `REBUILD_FACTOR` (1.5x). `TlasNode` is the 32-byte GPU node.
- `sidecar::{encode, decode, file_name}`: versioned side-car bytes (SHA-256 checksum,
  structural validation, rejects other meshes and builder versions).
- `BUILDER_VERSION`: bump whenever a change alters built bytes (it keys side-cars).

## Guarantees

- Deterministic: single-threaded, no hashing order, total-order sorts with index ties. The
  same mesh bytes give the same BLAS bytes (`builds_are_deterministic...` test).
- Conservative: quantized child boxes are rounded in exact (f64) arithmetic; clipped
  fragment boxes are widened by a few ulps so they contain the true clipped polygon. Every
  hittable triangle is reachable through boxes that contain it (`every_triangle...` test).
- Bounded stack: binary depth <= `3 * (TRACE_STACK + 1)` (median splits take over before
  SAH splits would exceed it); if the optimal collapse is still deeper than the stack, a
  breadth-first grouping (>= 3 binary levels per wide level) replaces it.
- Degenerate (zero-area or non-finite) triangles are left out (`stats.degenerate`).

## Test

- `cargo test -p ara-bvh`: reachability, CPU traversal vs brute force (flags, t ranges,
  filters), determinism and side-cars, deep unbalanced meshes, TLAS walk and refits.
- `cargo run --release -p ara-bvh --example fixture_stats -- <r4-1 dir> [set]`: builds an
  R4-1 lab fixture, checks the lab's independent references and compares CPU traversal
  time against tinybvh's own tree (a tree-quality proxy).

## Gotchas

- Never change the node or triangle layout without changing `shaders/trace.slangh`,
  `look/trace/gpu.rs` and `BUILDER_VERSION` together.
- Triangle word 3 is `[submesh, flags, 0, primitive]`; flag bit 0 = the plane row's
  dominant normal axis is negative (front faces need it).
- Barycentrics `u`, `v` weigh corners 1 and 2 (DXR convention).
- The SBVH's leaf triangles are whole triangles inside clipped boxes: a hit point always
  lies in some box of that triangle, but not necessarily in every leaf that holds it.
