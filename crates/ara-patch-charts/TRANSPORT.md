# Render-v3 transport data (A2)

This is a data layer, with no solver, direction pattern, readout, media grid,
rendering/default change or quality-gate promotion.

## Charts and import/load service

`ara_patch_charts::build(Mesh { positions, triangles }, Settings::default())`
constructs 25 cm cells at unit scale. Set `Settings::scale` to the signed instance
scale for a metric variant. Rotation and translation are applied afterwards by
the renderer; they do not rechart. Shear must be baked into a new mesh input.
Instances can share an immutable `Arc<Charts>` for the same content/scale key.

Clipping uses fixed-order f64 arithmetic, then rounds final records to FP32.
Cell/material/normal-hemisphere ordering and edge-connected components determine
patch order. Adjacent faces only merge when their normals meet the crease cosine.
Six normal hemispheres prevent a tiny closed curved object becoming a single
zero-normal patch. Material seams and disconnected pieces remain distinct.
This may create small boundary/crease patches; it never merges to a coarser grid.
The 25 cm quantity is cell width, not a requirement that every triangle edge be
25 cm: a planar square's diagonal is 35.36 cm, and a 3D cell's diagonal is 43.30 cm.
Fragments retain barycentrics and the **source triangle index**, before BVH reordering.
Algorithm version 2 also discards only cell residues whose accumulated area rounds
to exactly zero in FP32 (below half its smallest subnormal); Polar has such clipped
boundary residues. A metre-scale regression retains the representable area while
preventing a zero-area receiver or rejection of the entire mesh.

`cache::Key::new(mesh, settings)` gives SHA-256 mesh/variant keys.
`cache::prepare(mesh, settings, cached_bytes)` validates a side-car or rebuilds.
Its `Prepared` contains `charts`, `key`, and optional `write_back` bytes. Publish
those bytes atomically below the project cache root at `key.relative_path()`.
The canonical binary has checksummed lengths/content and no native padding.
Missing, old, wrong-scale or corrupt side-cars rebuild at the same resolution.

`ara-importers/src/mesh/patches.rs` provides `PatchCache`, `import_charts` and
`load_or_build` for a source material primitive. The service supplies I/O; these
functions do not read environment/profile state or change ARAF artifacts.
**Integration owner:** call/re-export the hook from the import/load service.
The owning brief permits only `mesh/patches.rs`, not `mesh.rs`, `lib.rs` or worker
services, so existing import jobs do not yet invoke it automatically.

## Wave 2 API

All renderer types are in `crate::look::transport::{patches,links,reverse}`.
Sibling `solve.rs`/`media.rs` can import them directly; no `mod.rs` re-export is needed.

1. Publish static/sleeping surfaces with
   `PatchRegistry::publish_traced(instance_id, trace_instance_key, key, Arc<Charts>, room_id, Placement)`.
   The key is A1's actual `InstanceKey`, not a mirror or truncated generation.
   `publish` is also available for callers with no tracing scene.
   Keep instance IDs stable across extraction order. `move_instance` preserves
   IDs and advances surface generation; `touch` invalidates cached appearance.
   `remove` on wake/removal returns a compact tombstone. Retain the registry's
   identity ledger until the owning scene is discarded. Reinsert/rechart advances
   incarnation, so old IDs never alias. Feed reset means discard the whole registry.
   Pass A1's `TraceScene::tombstones_since` entries to `retire_trace`. It checks the
   exact dead key, so a late tombstone cannot remove an already-published replacement.
   Active bodies own no persistent link rows.
   Surface/source/geometry generations are u64, matching A3. Chart incarnation is
   a checked u32 local counter; exhaustion returns an error rather than wrapping.
2. `Arc::new(registry.snapshot()?)` produces room/instance/local ordered FP32 SoA
   `PatchBuffers`: position xyz, normal xyz, albedo rgb, area, material, stamps and
   contiguous room ranges. `index(PatchId)` resolves stable IDs into this snapshot.
3. `LinkStore::build(snapshot, external_sources, expected_links, row_callback)`
   calls `row_callback(receiver_u32)` in snapshot order. Return `Vec<Candidate>`:
   ordinal (unique per receiver), source u32, FP32 weight, `Visibility` and optional
   exact sample segment. Source indices below patch count refer to patches;
   remaining indices refer to `ExternalSource` entries. Caller-defined ordinals
   allow either global or per-patch direction schemes. `Candidate::traced` accepts
   any `VisibilityQuery` implementation, including a CPU `trace()` adapter.
4. Store visibility RGB even when zero. Include every relied-on geometry stamp
   (opaque blockers, alpha/transmission layers, etc.). The pool rejects conflicting
   generations for one primitive in a build. Endpoint stamps/positions are owned by
   the immutable snapshot; geometry stamps are interned losslessly. Centroid paths
   are implicit; arbitrary sample endpoints use a sparse exact FP32 override array.
   `GeometryKey::from_trace(&scene, instance_key, primitive_key)` checks A1's mesh,
   submesh, source-triangle and liveness before caching `(InstanceKey, triangle)`.
   Each `GeometryStamp` adds A3's u64 motion/appearance revision. `Generations::geometry`
   must return `None` for dead A1 keys, as well as comparing the current revision.
   Triangle numbering must match the exact BLAS input. A1's native adapter uses
   LOD0 indices; rechart/rekey that order if import optimization changed it.
5. `ReverseLinks::build(&links)` gives source-to-link CSR.
   `dependencies(source)` returns forward IDs; `links.receiver(link)` resolves
   receivers. `patch_links(&links, patch, callback)` gives both incoming and outgoing
   dependencies without duplicates.
6. `CrossingIndex::build(&links)` gives region/geometry discovery.
   `region(&links, Region::new(min,max)?, callback)` finds exact segment/AABB crossings.
   Query both old/new swept bounds from A3, including newly inserted occluders.
   `primitive(&links, GeometryKey, callback)` finds exact recorded dependencies,
   including tombstoned primitives. `links.stale(link, &impl Generations)` checks
   both endpoints and all geometry stamps. **Stamps alone cannot detect new blockers.**
7. Solver uploads use `links.{source,weight,transmission,witness,groups,tiles}`.
   For receiver lane `l` in a tile, group `g` has its sample when bit `l` is set.
   The link index is `g.first + popcount(g.lanes & ((1u << l) - 1))`.
   Slot groups interleave up to 32 receivers; ragged rows need no padding. Tiles
   never cross room boundaries. Patches and RGB remain FP32, indices u32.

The graph and index are immutable publication units. Rebuild affected resident
sectors and publish them together; keep old snapshots until readers finish.
Wave 2 owns generation-marked scheduling, incremental splice/publication policy,
visibility service, solver, debt and channel invalidation. This API does not run a
repair queue or full-scene solve. The `expected_links` argument reserves storage;
it never silently drops links.

## Index design and accounting

The forward payload is 24 bytes/link: source 4, weight 4, visibility RGB 12,
witness reference 4. Groups cost 12 bytes per occupied slot (32 links when full),
tiles 16 bytes per 32 receivers. Arbitrary sampled endpoints cost 28 bytes per
override. External sources and every vector's actual capacity are counted.

Reverse CSR is `4 * (links + sources + 1)` bytes. It stores link IDs, not redundant
receiver/link pairs. The region index is an implicit complete binary tree over
pairs of consecutive coalescing groups (up to 64 links); each 32-byte node contains
bounds and a geometry bloom summary. Leaves derive their group range from heap
position, with no per-link or group-permutation array. Direct group iteration
avoids a receiver binary search for every segment during construction/query.
Exact segment and witness checks remove all broad-phase false positives.
Short, blocked and clear links all remain indexed. Worst-case discovery is linear;
this memory saving is not a claim that G3's event-time limits pass.

Geometry stamps and multi-primitive witness tables are charged to the index.
The actual A1-backed geometry stamp is 32 bytes. Singleton witnesses reference a
unique primitive directly. Multi-layer sets are
interned. Many unique multilayer paths can exceed 8 B/link; arbitrary per-sample
endpoints can exceed the 256 MB link ceiling. `Memory::measure` reports the actual
layout, with no fallback or quality reduction to hide a failure.

`Memory::within_g3_data_lines` uses the stricter decimal 8 B/link, 256 MB link-data,
512 MB total-array lines. Add retained chart coverage, build scratch, old/new
publication overlap, GPU copies, channels and all other transport residency for
the real peak-memory gate. No complete G3 pass is claimed from array accounting.

## Reproducible benchmark

After the shared quiet-window check, with `CARGO_BUILD_JOBS=4`:

```
cargo run -p ara-patch-charts --example transport_data -- --synthetic 48
cargo run -p ara-patch-charts --example transport_data -- --synthetic 48 --witness-stress
cargo run -p ara-patch-charts --example transport_data -- PATH_TO_SCENE.glb 48
```

The example directly compiles the owned renderer data modules and A1's actual
CPU identity model, ensuring the measured layout is the implementation. It parses default-scene transforms,
builds/caches per-mesh metric variants, reports edge/area distributions and times,
then uses a declared synthetic visibility model with 48 links/patch. It does not
claim physically selected link counts or real visibility/lighting quality. Source
buffers are released before constructing links. Run logs and measured values are
recorded in the A2 report under `D:/AraBuild/render-v3/measure/`.
