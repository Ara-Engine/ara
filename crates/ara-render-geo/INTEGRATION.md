# Rendering-at-scale handoff

This is a working standalone Vulkan rendering slice. It is **not yet the full
requested engine integration**: `ara-render-fidelity` and the editor are untouched.
The benchmark preview uses simple directional shading. It does not establish UE5
visual parity or production material coverage. Texture motion is measured
separately from the frozen-camera geometry/VSM warm timings.

## Assets and preparation

`ara-geo::cook(&SourceMesh, Settings)` emits 64-vertex/128-triangle meshlets,
border-locked grouped simplification, cumulative errors, bounds and independently
meshopt-compressed pages. `Geometry::{encode,decode,identity,cluster}` own ARAG v1.
`ara.geo` is installed in the builtin `ara-importers` worker registry. Its
`VirtualGeometrySettings` are reflected; importer version is
`2-settings1-meshopt0.7-dag1`. glTF/FBX v4 automatically emit one versioned ARAG
Custom artifact per static material primitive. MeshSettings.virtualGeometry
defaults true and opts out explicitly. Skin/morph meshes stay classic with a
manifest note. virtualGeometry in each mesh manifest links artifact/cacheVersion
and quality diagnostics (occupancy/reduction, slivers, degenerates, broad triangles,
material fragmentation). Import action versions invalidate prior cache entries.
The current plugin input is an extracted JSON SourceMesh, not a new source reader.

`GeometryView::new` prepares a fully resident snapshot. `new_with_pool` takes
`GeometryConfig` with an explicit physical page budget. Instance transforms are
positive uniform scale, yaw and translation. identity.w encodes yaw radians as
f32 bits; identity.z is a virtual albedo ID (u32::MAX skips texturing). Nonuniform
scale, arbitrary rotation, masked/deformed materials and material-AST evaluation
remain integration work. UVs use stable perspective interpolation/derivatives.
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

The fidelity owner's current contract is README.md#geometry--shadow-integration-contract
in ara-render-fidelity. WORLD/NORMAL/BASE/MOTION/DEPTH/DIRECT and FILTER0 indices
and Frame.read [[u32;4];7] remain untouched. This producer's channels above are
**not that schema**: integration needs the owner's additive graph adapter, stable
ray-primitive correspondence and shared material evaluation. No fidelity code is
edited in this round. VirtualShadows::output remains RGBA32F RGB PCF visibility,
A all-taps-resident validity, same render size, Graphics/ShaderRead. It may replace
direct-light visibility; do not multiply indirect lighting by the mask.

Fidelity round 3 adds `SunShadowSource::VirtualShadowMap { visibility: ImageInput,
render_size }` without an ara-vsm dependency. A>=0.5 is valid/resident; invalid
pixels trace the scene. Only the primary-surface sun uses this mask; local and
secondary rays retain RT. Borrow images through completed render and preserve
Graphics/ShaderRead at input/output. Query the upcoming `render_jitter(&FrameInput)`
and use matching render size, camera and jitter in the producer. The standalone
benchmark remains unjittered; the host adapter must apply the exposed jitter.
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
viewport-menu wiring and stats panels. All authoring changes must still use
typed commands/ops; these renderer objects are derived snapshots only.

## Texture pages, typed debug settings and HDR

`ara-texture-stream::Source::{rgba8,from_cooked}` prepares complete mip chains;
the latter consumes validated BC7/BC4/BC5 KTX2. `Catalog` assigns mip-major tile
addresses. `Residency::new` allocates a byte-budgeted software virtual texture
pool: 128-square interiors, one wrapping texel of gutter, resident RGBA8 pages.
This is atlas virtualization, not Vulkan sparse memory. Compressed source media
is decoded off the frame thread. HDR/ASTC source textures need a format adapter.

At a completed slot boundary call prepare, submit begin_frame, pass binding to
GeometryView::bind_textures and/or Landscape::bind_virtual_textures, render all
consumers, submit end_frame and call submitted. Pool ownership stays with the
host. All consumers declare Binding.resources; they sample with the same shader
helper, using analytic mesh derivatives or triplanar fragment derivatives.
Timestamped page requests and min-mip feedback are combined within subgroups.
Compute/fragment basic/vote/ballot/arithmetic support is checked before pipelines.
Pinned coarse mips guarantee fallback; priority/LRU only evicts leaves, and child
publication requires a parent resident in an earlier frame. Cold starts/camera
cuts still need prefetch time. Stats exposes pool, staging/feedback and CPU source
bytes, resident/uploaded pages, sample lag and misses. Diagnostics capture UV,
desired and sampled mip. Motion reports state camera speed and correspondence;
stable triangle/UV matches avoid conflating distant surfaces sharing one atlas UV.

`settings::{RenderSettings,register_schema}` reflects debug_view (lit, clusters,
LOD, overdraw, raster, VSM pages, texture residency), pixel error, texture budget
and ara-color::OutputSettings. This is the editor extraction seam; it performs no
side-channel document edits. The VSM debug preview shows cached page coordinates,
red dirty pages and magenta missing pages. Texture debug red means >1-mip lag.

DisplayOutput consumes exposed linear BT.709, applies a highlight shoulder then
SDR sRGB, HDR10 BT.2020/PQ or linear scRGB. Present::new_with_output selects an
advertised matching swapchain, and display_encoded rejects mismatched modes.
Paper white defaults 203 nits; SDR remains default. See HDR.md for the manual
display gate and actual queried format/encoded-value evidence in captures/hdr.
