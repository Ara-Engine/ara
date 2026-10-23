# Terrain and foliage integration

`ara-terrain` owns layered terrain documents, binary tile payloads and cooking.
`ara-foliage` owns seeded scatter rules and manual paint documents.
`ara-landscape` records native GPU passes through `ara-render::Graph`.
The only shared runtime addition is `ara-gpu`'s validated vertex-pulling
`DrawIndirect` command and `Access::Indirect` barrier state.

## Authoring and editor tools

Register both document schemas and command catalogues in the host's shared
`SchemaDb`/`Registry`. These registrations supply inspector descriptions, typed
CLI/MCP parameters and the command palette; no editor-specific mutation path exists.

```rust,ignore
ara_terrain::register(&mut db)?;
ara_foliage::register(&mut db)?;
ara_terrain::commands::register(&mut registry, &mut db);
ara_foliage::commands::register(&mut registry, &mut db);
```

Commands:

| Terrain | Foliage |
|---|---|
| `terrain.create` | `foliage.create` |
| `terrain.layer.add`, `terrain.layer.set` | `foliage.rule.set` |
| `terrain.raise`, `terrain.paint`, `terrain.hole` | `foliage.paint` |
| `terrain.stamp`, `terrain.erode`, `terrain.import` | `foliage.exclude`, `foliage.erase` |

Send all pointer samples from a gesture as one `dabs`/`points` array in one batch.
The resulting transaction retains attribution, base revisions, dry-run, automatic
inverse ops and per-author undo/redo. Sculpt/stamp/erosion/import operations remain
ordered layer records; tile records receive only touched-tile reference deltas.
Layer priority, opacity and visibility remain editable without destroying source
gestures. Imported heights/weights/holes and stamps use immutable cache hashes.
Terrain manifests are written with `ara_doc::write`; ARTH v1 tiles use validated
little-endian binary payloads and BLAKE3 content keys.

`Terrain::evaluate` runs on a worker. `BakeJob` starts `ara-terrain-worker` below
normal priority on Windows and polls without blocking on compilation or cache reads.
Attach the source revision to each job, reject obsolete results, retain the last
good tile on failure, and apply only at a host-approved frame/tick boundary.
`Residency` bounds CPU tile storage; the renderer has 256 detailed GPU pages plus
an always-resident coarse fallback. `queue_tile` permits one 13,084-byte upload
per frame. Cache prefetch never enters the frame loop.

## Rendering and assets

CDLOD uses fixed 32-cell patches and six GPU-selected levels. Fine edge vertices
collapse onto the coarser neighbor's grid and read the same global samples. This
avoids skirts and works across arbitrary adjacent level gaps. Nonresident pages
use the resident 8 m grid; detail pages contain 2 m samples in the scale scene.
Selection, frustum culling, compaction and indirect arguments run on GPU.

The checked `ara-material` AST generates four-channel normalized splat blending
and triplanar surface evaluation. Terrain and foliage cast directional shadows.
Foliage shares wind/alpha/dither evaluation across depth, shadow and colour passes.
Current terrain depth builds a conservative minimum-depth HZB; large projected
bounds fall back to visibility. LOD pairs use complementary screen-door fades.

The sample's grass, conifer trees and rocks are original procedural stand-ins.
For project assets, resolve `Rule.asset` into `FoliageMesh::from_cooked` LODs,
provide baked alpha billboard impostors as the third LOD, call
`FoliageGeometry::apply_bounds` on extracted instances, and install the geometry
on a prepared landscape snapshot. Asset texture alpha is used in every raster
pass; sRGB colour is decoded once. Static render extraction owns no gameplay state.

`Landscape::record` performs no I/O, cooking, pipeline creation or GPU wait.
The host owns completion of frame slots and submission of each returned command
list. Scene creation, asset uploads and PSO warming are preparation operations:
schedule them outside the live editor frame path and publish under D20 policy.

## Physics, navigation and fidelity

`Tile::cook_collider` now uses merged `ara-physics`: solid tiles create native
Jolt heightfields; cell-hole tiles create exact triangle meshes because Jolt's
vertex-hole convention removes adjacent cells. Fully empty tiles return `None`.
Cooked geometry includes the world origin and uses a static identity-pose body.
Publish new handles only at permitted fixed ticks, with bodies/state owned by ECS.
Native ray tests cover tile seams, world offsets and the precise hole footprint.

`Heightfield::mesh` produces upward-wound, hole-aware world vertices/triangles.
Assign them to `ara_nav::Geometry` and provide area 0 per triangle. Pass dirty tile
bounds and navigation-profile halos into incremental baking. The same mesh and
content keys are the GI/RT acceleration-structure inputs. `Landscape::outputs`
exports depth, shadows, HZB and SDR colour for host composition. The fidelity track
owns richer lighting/probes/DDGI; the sample currently uses an ambient fallback.

## Verification and evidence

Run `./run.ps1 -Mode Check` for crate checks, tests, clean clippy and formatting.
It uses two Cargo jobs and scopes the Vulkan implicit-layer workaround and SDL
runtime PATH to the process. `-Mode Cook` generates/validates SPIR-V and DXIL.
`-Mode Capture` records the 4096 m square at 2560x1440 with over 1.39M instances,
asynchronous cache prefetch, bounded tile uploads and per-pass GPU distributions.

- [Noon golden](tests/goldens/noon.png), [dusk golden](tests/goldens/dusk.png).
- [Device/content provenance](tests/goldens/provenance.json).
- [Per-pass p50/p95/p99 timings and visible counts](measurements/gpu-timings.json).

Goldens use local RGB 1-SSIM <=0.005 and separately require identical repeated
fixed-tick captures. Other tests exercise command rollback/undo/redo, layer
composition, erosion borders, every CDLOD edge gap, seeded scatter partitioning,
worker publication, actual streamed edits, authored meshes and indirect validation.
These are serialized diagnostic timings, not an end-to-end editor FPS guarantee.

The CPU terrain format supports signed tile coordinates and 16..256 cells. The
current GPU extraction accepts square origin-zero terrains with 32-cell tiles.
Erosion cooking diagnoses dependency halos above 256 samples; larger stacks need
checkpoint support, while their source gestures remain editable. Metal execution
depends on the GPU backend track; both offline shader targets are supplied here.
