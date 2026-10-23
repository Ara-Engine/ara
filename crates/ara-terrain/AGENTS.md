# ara-terrain

Owns layered heightfield manifests, immutable binary tiles, stroke transactions,
worker evaluation, CDLOD reference rules and bounded last-good tile residency.
No ECS/gameplay state. Y-up metres; rows increase along +Z; tile bounds are half-open.

Public API: `register`, `commands::register`, `Terrain`, `Tile`, `FileCache`,
`TileSource`, `Residency`, `Heightfield::mesh`, `lod`.

Every live edit goes through ara-cmd. A batch is one gesture/undo step; include all
dabs in that gesture. Each touched tile stores a small ordered stroke-reference
delta, not its sample array. Layers sort by (priority, ID); gestures by (sequence, ID).
Imported bases/stamps are immutable ARTH v1 little-endian binary payloads keyed by
BLAKE3. Canonical manifests use ara-doc, never a custom JSON writer.

`evaluate` is WORKER ONLY. Its erosion dependency halo makes independent tiles
agree at borders. It diagnoses dependency halos above 256 samples rather than
allowing unbounded worker allocations. Larger erosion stacks need checkpoint
support before cooking; their source gestures remain intact.
`ara-terrain-worker MANIFEST CACHE X Z OUTPUT` performs one out-of-process cook.
`BakeJob` spawns below normal priority on Windows, polls without waiting and
decodes payloads on an I/O thread. Hosts retain old results on
failure and call Residency::publish only at a permitted boundary. No authoring
change is allowed to create a blocking cook on the frame/tick thread.

Physics uses `Tile::cook_collider` over ara-physics. Solid tiles cook native Jolt
heightfields; tiles with cell holes cook exact triangle meshes, because Jolt's
vertex-hole convention removes additional neighboring cells. Empty tiles return
None. Geometry includes the world origin; bodies use the identity pose. Native
ray tests verify adjacent heightfields and the exact hole footprint. Navigation consumes
`Heightfield::mesh`: assign vertices/triangles to ara_nav::Geometry and area 0 per
triangle. Dirty tile bounds plus profile-radius halos drive incremental baking.
GI/RT/shadow consumers use the same hole-aware mesh and tile content keys.

CDLOD uses fixed tile patches and coarse-neighbor edge collapse, not skirts. The
GPU implementation is in ara-landscape. Tests compare the CPU edge rule and
independently evaluated adjacent tiles.

Set CARGO_BUILD_JOBS=2. Gates:
`cargo check -p ara-terrain`
`cargo test -p ara-terrain`
`cargo clippy -p ara-terrain --all-targets -- -D warnings`
`cargo fmt -p ara-terrain -- --check`
