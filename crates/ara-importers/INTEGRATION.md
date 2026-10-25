# Asset pipeline integration

The new crates are picked up by the workspace's existing `crates/*` glob. No root
manifest or existing crate changes are needed. Add explicit path dependencies
where integrating. The parent track should regenerate Cargo.lock when integrating;
this branch commits only its owned crates.

## Host / protocol

`ara-asset` owns `AssetPipeline`, `ProcessWorker`, `Importer`, `ImportJob`,
`FileCache`, `CacheBackend`, `DependencyGraph`, sidecar commands and stable slots.
`ara-importers` supplies `GltfImporter`, `FbxImporter`, `TextureImporter`,
`MeshSettings`, `TextureSettings`, `Vertex`, `cook_mesh`, `cook_texture` and
`registry()`. `ara-import-worker` installs that registry and serves one request.

Register `ara_asset::register_schema(&mut db)` before reading sidecars. Each sidecar
uses the shared schema DB. `ara_importers::registry()?.register_schemas(&mut db)`
also registers MeshSettings, TextureSettings and TextureUsage (defaults, ranges
and docs); plugins implement Importer::register_settings for their own types.
Each sidecar is an import document with the source asset ID and one `i_settings` record of
reflected type ImportSettings. It carries importer ID/version, final CookPreset, JSON settings and
`@path#id` dependencies. `SidecarCommand::Initialize/Settings/Preset/Dependencies::edit`
returns ordinary kernel Edits: submit them with author/base/why through ara-ops,
then persist `ara_doc::write` output. This gives sidecars kernel ops, inverse edits,
undo, revisions and canonical key/default behavior. Importers never modify them.
`Sidecar::index` indexes the media path, not the `.import.json` path; the existing
CLI's sidecar scan already reads their top-level ID. AssetIndex repairs stale paths.

Create `FileCache::new(project)`, then `AssetPipeline::new(project, worker, cache,
schema)`. Register SourceAsset entries with sidecar identities and exact importer
versions; request initial imports. `poll()` drains native filesystem changes,
snapshots files off-thread, discovers dependencies in a worker, then checks cache
or cooks in a worker. External buffers/images and declared asset hashes participate
in input identities. Renames/deletions and bad edits report failure without
replacing the live slot. Events arriving during first dependency discovery are
retained and reconsidered once dependencies are known.

Use `pipeline.apply_boundary(idle, playing, approved_assets)` at a frame/tick
boundary. It emits AssetChanged only after an entire valid generation is ready and
invalidates cooked-input consumers. `reload.policy` supports immediate, idle or
approval application; Play blocks swaps by default. Explicit per-asset approval or
`allow_during_play` is required to change a running human Play session. Old CPU
generations are freed on a background reclaimer, not during the frame swap. The
renderer must retire its old GPU resources after the relevant fences.

`ReloadEvent` maps to protocol job progress/ready/failure; `ImportJob::poll` is also
available for custom scheduling. Cancellation kills the native child, deadlines
terminate hangs, and superseded epochs cannot publish. IPC is length-prefixed
MessagePack (u64 LE length, maximum 512 MiB), with binary source/artifact buffers.
Dependency discovery is also isolated: native FBX parsing never runs in the host.
Windows children are hidden and below normal priority; background host threads use
lower priority. Linux nice and macOS utility QoS paths are present but not tested
on this Windows machine. No persistent daemon or service is installed.

FileCache separates immutable BLAKE3 objects from action manifests under
`.ara/cache`, with its own ignore marker so cache contents stay out of git without
changing the project's root ignore file. It verifies hashes and cooked formats on load. Conflicting results
for one action are rejected as corruption/nondeterminism. A shared cache server
implements CacheBackend. Collect only with writers quiescent, including roots from
all active/staged/undo generations and every process. Snapshot and cache APIs do
disk I/O; call them from background/CLI code, never directly on an editor frame.

## Progressive quality and execution budget

Set `ImportInput.preset` (or `SourceAsset.preset`) to the final project/export
quality. `CookPreset::HighQuality` is the serde/Rust/schema default. Fast selects
ISPC BC7 ultrafast / BC6H fast; Balanced selects basic / basic; HighQuality selects
slow / very slow. `SidecarCommand::Preset` produces an undoable kernel Edit.
Exports call `ProcessWorker::start` with the final preset directly, or explicitly
set `HotReload.progressive = false`; export code must not consume a preview slot.
The v1 mesh/skeleton/clip/KTX2 contracts and renderer/animation readers are unchanged.

`HotReload.progressive` defaults true. On a final-cache miss it cooks/loads Fast,
stages a complete generation before disk persistence, then cooks the requested
final preset. Slow cache writes never hold up the first preview publication. `Ready` now
has `final_pass`: false means preview readiness, true means the requested final
quality is ready. Jobs remain active across preview publication; protocol adapters
must not report completion on the preview event. Both generations use existing
frame/idle/approval/Play gates, epochs and last-good fallback. HQ failure retains
the preview or prior live generation. A final-cache hit never schedules/downgrades
to Fast. Staged results superseded by HQ are freed off-thread too.

`ImportInput.thread_budget` / `SourceAsset.thread_budget` defaults to 16 and accepts
1..=256. One private Rayon pool bounds nested texture/mip/block and mesh tasks.
Budgets do not enter the cache key; the preset does. Action keys are now v2 and
builtin importer versions are bumped. Use the current `Importer::version()` when
creating/updating sidecars through kernel ops. Worker processes retain their
below-normal Windows process priority (Linux nice / macOS utility setup also
remains). In-process registry calls are CLI/benchmark APIs and do not change the
host process priority. Never invoke them from an editor frame.

Decode/pixel preparation, mip filtering and BC packing are parallel. Mesh/primitive
optimization is parallel; native ufbx extraction is followed by owned Rust jobs.
Texture batches retain at most eight images' float/mip buffers before encoding.
Four-pixel-high block-row tasks bound scheduling tails; output order is fixed.
Frequently repeated solid BC7 blocks reuse exact ISPC results, including alpha;
no quality heuristics replace the selected encoder. Alpha mode is selected for
an entire mip. All pixel reductions keep their original per-pixel operation order.
`ImportContext.metrics` optionally collects elapsed batch-phase timings and never
enters cooked bytes. `progress` is now Send so bounded pool execution can own it.
Direct `cook_texture` remains HQ; `cook_texture_with_preset` accepts explicit quality
and uses the current Rayon pool (wrap direct calls in a private pool if needed).

## Renderer / animation

Depend on `ara-asset-format` for small runtime readers; source encoders stay out of
the runtime dependency graph. [FORMAT.md](../ara-asset-format/FORMAT.md) is the v1
contract. Mesh/clip/skeleton readers borrow original storage without allocating
payloads; Texture borrows mip bytes and parses only small KTX2 metadata.

Mesh has 52-byte quantized vertices, u32 LOD indices, source vertex mappings,
eight skin influences, and 64-vertex/124-triangle meshlets with sphere/cone bounds.
meshoptimizer optimizes vertex cache/fetch and simplification LODs. MikkTSpace
generates missing tangents, splitting mirrored UV seams before dedup/remapping.
Upload the byte slices directly; create a vertex decode path for UNORM positions,
SNORM normals/tangents and half UVs. Meshlet and index sections are ready for a
GPU-driven path. Source-vertex mappings address original glTF morph targets.

Static classic chains default to eight levels and a relative error ceiling of
0.1. Targets halve triangle counts until reaching 2% of LOD0 or 200 triangles,
whichever comes first. Levels must shrink by at least 25%; error is nondecreasing.
Attribute-aware meshoptimizer weights normals, tangents, UVs and colour, protects
UV/colour/handedness seams, and locks geometric open borders even when source
vertices are split. Material primitives are simplified separately. Mask/blend
glTF materials (including companion opacity maps) retain at least 90% of each
connected component's area, including rotated cards with coincident backfaces.
FBX and material-unknown `cook_mesh` calls apply
that guard conservatively. Border/seam/error/area constraints can stop a chain
early; a leaf already represented by two border triangles cannot shrink safely.
Skin/morph meshes retain the previous four-level, 0.01 error ceiling.

ARAF v1 `Lod.error` remains relative to the model extent. The native adapter
converts it to model metres once, then runtime LOD selection applies instance
scale and projected pixels per metre. Attribute error is included conservatively;
no runtime format or selection threshold changes accompany these deeper chains.
glTF/FBX generation 6 invalidates prior cooks through the normal importer version
and sidecar upgrade path. Re-cook through the worker rather than editing caches.

The canonical `scene.json` artifact maps source mesh primitives, images/usages,
skins and clips to stable artifact names. glTF's source document retains
scene hierarchy, PBR factors, samplers, texture transforms, extensions, cameras and
lights; it is metadata, not a request to reopen source media at runtime. Match a
material texture's source image index plus usage to the cooked texture mapping:
base color/emissive → color, metallic-roughness/transmission/AO → linear, normal → normal.
An image used in several color spaces receives distinct cooked artifacts. FBX has
an equivalent explicit node/material/primitive mapping with ufbx typed IDs.

Skeleton joint order matches mesh indices. Nodes retain non-joint ancestors;
evaluate that hierarchy before applying inverse binds. Clip node IDs use the same
scene indices, preserve glTF step/linear/cubic tracks and morph weights, and use
TRS xyzw conventions described in FORMAT.md. Fox validates one skin and three
clips. The animation track can add importers by registering another Importer in a
worker executable; its cooked bytes can use these formats or ArtifactKind::Custom.

Builtin importer IDs/versions are returned by Importer::id/version. Version them
whenever algorithms or encoder behavior change. Settings are declarative: meshes
accept lodCount/lodError; textures accept usage, mipmaps, flipNormalY, maskChannel
and maxDimension. Desktop targets emit BCn. Mobile encoding is a plugin seam;
the shared format reader already supports ASTC 4×4/6×6/8×8.

glTF generation 5 also accepts `opacityMaps`: image URI (or `#0` for an embedded
image index) to a companion opacity URI relative to the model. The red channel
multiplies existing base-color alpha; mismatched dimensions use triangle filtering.
Only formerly opaque materials switch to alpha mask. Source/dependency bytes stay
immutable. `dependencies_with_settings` transports these companion files through
the process worker into the frozen snapshot and its action digest. FBX rejects
nonempty opacity maps; its generation also advances for the shared settings ABI.

Runtime `ModelMaterial` exposes independent UV transforms/selectors for color,
normal, metallic/roughness, occlusion, emissive and transmission. Coordinates follow
`offset + rotation * scale * UV`; both original UV sets survive cooking. Missing
tangents use the selected, transformed normal coordinates, while supplied tangents
remain authored. Emissive strength folds into the emissive factor exactly once.
Transmission retains a factor and linear red-channel texture independently of
alpha coverage. These follow the [Khronos texture-transform specification](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_texture_transform)
and [core tangent guidance](https://github.com/KhronosGroup/glTF/blob/main/specification/2.0/Specification.adoc#using-primitive-data).

## Encoder choice

Adopt pinned `intel_tex_2 = 0.5.0`, the maintained bindings to Intel's
[ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor),
with precompiled Windows/Linux/macOS kernels (no ISPC compiler prerequisite).
It covers BC4/5/6H/7 through one CPU implementation, uses explicit fast, balanced and high-quality
presets and gives deterministic worker output. The independent decoder is
texture2ddecoder. See [MEASUREMENTS.md](MEASUREMENTS.md) for PSNR and timings.

The older ispc-texcomp/intel_tex wrappers offer the same underlying algorithm with
older bindings; image_dds adds DDS-oriented layers around this backend. GPU
block_compression ports Intel kernels to WGSL/wgpu, adding a device dependency to
imports and competing with the human's renderer. CPU ISPC fits isolated workers
and D20. This evaluation selects a strong reproducible baseline; it does not claim
an exhaustive speed/quality comparison against every commercial encoder. A future
encoder replacement must be measured, versioned and quality-tested on real images.
