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
reflected type ImportSettings. It carries importer ID/version, JSON settings and
`@path#id` dependencies. `SidecarCommand::Initialize/Settings/Dependencies::edit`
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

The canonical `scene.json` artifact maps source mesh primitives, images/usages,
skins and clips to stable artifact names. glTF's source document retains
scene hierarchy, PBR factors, samplers, texture transforms, extensions, cameras and
lights; it is metadata, not a request to reopen source media at runtime. Match a
material texture's source image index plus usage to the cooked texture mapping:
base color/emissive → color, metallic-roughness → linear, normal → normal, AO → mask.
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

## Encoder choice

Adopt pinned `intel_tex_2 = 0.5.0`, the maintained bindings to Intel's
[ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor),
with precompiled Windows/Linux/macOS kernels (no ISPC compiler prerequisite).
It covers BC4/5/6H/7 through one CPU implementation, uses slow BC7 / very-slow
BC6H presets and gives deterministic worker output. The independent decoder is
texture2ddecoder. See [MEASUREMENTS.md](MEASUREMENTS.md) for PSNR and timings.

The older ispc-texcomp/intel_tex wrappers offer the same underlying algorithm with
older bindings; image_dds adds DDS-oriented layers around this backend. GPU
block_compression ports Intel kernels to WGSL/wgpu, adding a device dependency to
imports and competing with the human's renderer. CPU ISPC fits isolated workers
and D20. This evaluation selects a strong reproducible baseline; it does not claim
an exhaustive speed/quality comparison against every commercial encoder. A future
encoder replacement must be measured, versioned and quality-tested on real images.
