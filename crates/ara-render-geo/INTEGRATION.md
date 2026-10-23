# Rendering-at-scale handoff

This is a working standalone Vulkan rendering slice. It is **not yet the full
requested engine integration**: `ara-render-fidelity` and the editor are untouched.
The benchmark preview uses simple directional shading. It does not establish UE5
visual parity, production material coverage, or performance under camera motion.

## Assets and preparation

`ara-geo::cook(&SourceMesh, Settings)` emits 64-vertex/128-triangle meshlets,
border-locked grouped simplification, cumulative errors, bounds and independently
meshopt-compressed pages. `Geometry::{encode,decode,identity,cluster}` own ARAG v1.
`ara.geo` is installed in the builtin `ara-importers` worker registry. Its
`VirtualGeometrySettings` are reflected; importer version is
`2-settings1-meshopt0.7-dag1`. glTF/FBX extraction can call cook for each opaque
material domain. Automatic glTF/FBX virtual-geometry side outputs remain unwired.
The current plugin input is an extracted JSON SourceMesh, not a new source reader.

`GeometryView::new` prepares a fully resident snapshot. `new_with_pool` takes
`GeometryConfig` with an explicit physical page budget. Instance transforms are
positive uniform scale and translation. Rotations/nonuniform scale, masked or
deformed materials, textures and material-AST evaluation remain integration work.
Geometry/instance replacement requires a new extraction; no authoring document
or gameplay state is mutated here. Publish extractions at the host's D20 boundary.

## Frame boundary and streaming

After the previous frame slot completes: optionally call `prepare_streaming`
off the interactive thread, then `update`, `record`, submit and call `submitted`.
The host owns waits and retirement. Discard/recreate an extraction if submission
fails after record advances its state. Never overwrite an in-flight frame slot.
The conservative HZB is a 16x16 minimum-depth tile level. Bounds spanning more
than four tiles per axis skip occlusion; a full multilevel pyramid remains work.

GPU feedback is copied to a persistent readback buffer; no CPU count drives draw
submission. Streaming uploads at most 16 pages per batch, removes evicted mappings
and publishes new mappings after the page copy. Roots are pinned in a deterministic
LRU pool. A complete asset level must be resident before a finer cut is enabled.
`Stats::resident_floor > 0` explicitly reports that residency can exceed the
requested pixel error. This conservative policy is not Nanite's fine-grained
dependency-aware streaming; replacing it is remaining scalability work.

## Deferred surfaces and fidelity owner

`GeometryView::outputs` returns same-device images in Graphics/ShaderRead:

| Channel | Packing |
| --- | --- |
| base_roughness | Linear base RGB, perceptual roughness A |
| normal_depth | World-space unit normal XYZ, positive view distance A |
| world_error | World position XYZ, world-space LOD displacement bound A |
| identities | Two U32 words: render instance index + 1, cluster * 128 + triangle |
| preview | SDR diagnostic image; not a fidelity lighting input |

Entity IDs and material indices come from the instance table. Benchmark JSON
maps compact render indices to stable U64 IDs.
For static instances, motion can be reconstructed from world position and the
current/previous view projections. Animated motion is not implemented.

The existing fidelity `FeatureInputs`/`post_process` seam consumes completed HDR
and cannot replace primary visibility or direct shadowing. The fidelity owner
must add an external surface input which skips its primary ray/raster visibility,
maps these channels into transport/history inputs, and accepts a sun/local-light
visibility image. Do not feed the SDR preview into the HDR seam. Keep the existing
RT option. No fidelity files or shader manifests were modified by this track.

## Shadows

`ara-vsm::{directional,spot,point}` produces sun clipmaps, one spot projection or
six cube faces. `shadows::VirtualShadows::{new,update,record,submitted,output}`
marks pages from the resolved surfaces, allocates 128-square pages in 16k domains,
clears/renders dirty pages through the same geometry fetch, and emits 3x3 PCF
visibility. `output` is Graphics/ShaderRead; RGB is visibility and A is residency.
Pass one light's projections to each view. Sun views support up to six levels.

Call update with reset on caster extraction changes. Light revisions, changed
projection matrices and streamed geometry revisions invalidate the GPU cache.
The GPU allocator uses a bounded clock scan protecting current requests; the
separate CPU Cache provides deterministic LRU and explicit completion generations.
GPU clipmap scrolling currently invalidates the entire cache, rather than reusing
world-page identities across a changed projection. Invalidation is conservative;
fine bounds-only invalidation remains work. The cold-cache cost is reported.
`ShadowReference` keeps an exact-source instanced BLAS/TLAS ray-query comparison.

## Captures and verification

Run `run.ps1 -Mode Check`, `-Mode Cook`, `-Mode Native`, or `-Mode Upscaled`.
`--hardware-only` and `--indirect` benchmark flags compare raster backends on the
same scene. Upscaling is explicitly linear-light bilinear, not FSR or temporal AA.
All timings use device timestamps. Capture waits and PNG I/O are excluded.
JSON includes every pass, cold/warm results, page memory, system VRAM at allocation,
queue overflow, raster share and stable entity mappings. The workstation is shared.

The committed hybrid golden is strict same-device output. Tests also check mixed
DAG cuts have two uses of every shared edge, one-pixel silhouette agreement with
leaf geometry on the two-instance fixture, exact repeat captures, mesh/indirect
coverage, page-budget fallback/uploads, sun-cache reuse/invalidation, local-light
pages, and VSM versus full-source RT visibility. This is not a measured global
Hausdorff-error proof for the billion-triangle scene or a temporal quality test.

Editor work remaining: extraction/publication, material lookup, picking translation,
debug-view capture routing and stats panels. All authoring changes must still use
typed commands/ops; these renderer objects are derived snapshots only.
