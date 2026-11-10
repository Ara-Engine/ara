# Moving Renderlab onto ara-gpu

Design proposal, 2026-11-10. Renderlab source audited read-only at
`71ea35542346c70a2eea0b48d8ef05a931876979`; Ara layer at PR #5, based on
`fc03dd421` plus the Metal heap hardening. No Renderlab implementation changes are
part of this document. This proposes a migration, not a renderer-default change.

Keep Ara's global bindless heap and 32-byte roots. Replace Renderlab's bind groups
with typed records containing heap indices and byte offsets, and generate barriers
from a frame graph. Start with **bloom `downsample`, bloom `upsample`, then `post
tone`**. Prove each through frozen-input replay before joining it to the candidate
frame. Keep the shipping wgpu renderer usable throughout; use separate renderer
instances for the eventual whole-frame backend A/B.

## What is being moved

The older “107 bind-group layouts, about 60 passes” describes workload scale, not
107 interfaces that should survive the port. At the audited source there are 29
`create_bind_group_layout` call sites across `src`, including tests, inferred layouts
beside them, and 50 pass-opening sites in ordinary frame modules after excluding
test/reference-only paths. Constructor maps, skinned-mesh instances, mip/face loops,
optional modes and repeated dispatches make live objects and executed passes vary.
Two frame compute scopes only stamp timestamps. Do not size heaps or graph nodes
from these lexical counts. [Layout composition][modules], [inferred atmosphere
layouts][at-inference], [skinning layouts][skin-layouts], [old counts][old-counts].

Renderlab currently uses Vulkan wgpu; Ara supplies the native device on the candidate
path. Keep backend selection separate from rendering algorithm selection:

- GPU backend: `wgpu_vulkan`, `ara_vulkan`, `ara_metal`.
- Render path: `hardware_rt`, `non_rt`; both retain all their existing features.
- Frame settings and authored scene/program inputs are shared CPU data, not GPU
  handles that happen to have equal numbers on two devices.

Preserve the actual orchestration: skinning → lights/local shadows → AS build →
selected conventional/transmission shadows → atmosphere → selected visibility and
sun visibility → GI → programmable shading → reconstruction → current transparency
branch → post → presentation. Film, Interactive, forward raster and visibility-buffer
modes branch inside that order. Some atmosphere/fog work waits for newly produced
Virtual pages. The traced, non-temporal path can submit visibility early and do CPU
preparation while it runs; preserve that useful split rather than forcing every
candidate frame into one giant submission. Preserve current transparency/upscale
branch behavior during the port, and track pre-existing feature holes separately.
[Early frame][frame-early], [deferred branches and early submit][frame-deferred],
[forward][frame-forward], [tail][frame-tail].

## Bindings become data

One small renderer module owns resource identity, persistent descriptor publication,
frame upload pages and typed pass records. Its interface accepts scene/pass inputs
and returns commands plus explicit resource uses. It does not expose wgpu bind-group
concepts to the candidate passes. Initially use concrete backend adapters at the
post-tail seam; avoid a generic facade copying wgpu's entire interface.

| Existing binding | ara-gpu representation | Lifetime / rule |
|---|---|---|
| `var<uniform>` camera, frame, light, material or settings | A record in a storage buffer; root/pass record carries buffer index + byte offset | Large records stay out of roots. Read via explicit `Load` helpers or an agreed Slang storage layout. |
| Read-only storage buffer | Storage-buffer heap index plus byte offset/count | Read-only shader helper and graph `ShaderRead`; native binding can remain RW byte-address storage. |
| Writable storage buffer / atomic queue | Same heap with explicit range/count | Graph `Storage`; order every producer/consumer phase, including WAW and counter clears. |
| A bind-group buffer slice or dynamic uniform offset | Buffer heap index + byte offset/record ID | Preserve original range/stride in the first port, then relax uniform-alignment padding deliberately. |
| Sampled 2D, integer, depth, 3D or 2D-array texture | Typed sampled heap slot from `sampled_image` or `sampled_view` | Dimension, sample count, numeric type and mip/layer view are part of the shader ABI. IDs use integer loads. |
| Storage texture | Storage-image heap slot, with one-mip `storage_view` when needed | Graph range is the selected parent mip/layers; a sampled alias is a separate slot, not another image allocation. |
| Ordinary sampler | Immutable cached sampler heap index | Deduplicate descriptions across materials; filtering, wrap and anisotropy stay explicit. |
| Comparison sampler | Index in the separate comparison table | Preserve LessEqual used by glass/particles; do not reinterpret as a regular sampler. |
| Texture/buffer/sampler binding array | A storage table of heap indices / buffer slices | Descriptor index is not the old array position. Mark divergent indices nonuniform in Slang. |
| TLAS | Acceleration heap slot; geometry/instance table beside it | Graph AS input/build/read edges; native instance custom IDs are limited to 24 bits. Keep larger tags in the instance table. |
| Vertex/index/instance streams | Native draw buffers plus storage indices if shaders also read them | Declare all uses and exact layouts. Keep nonzero vertex/index offsets and first-instance behavior. |
| Indirect draw/dispatch arguments | Buffer + byte offset in existing indirect commands | `Storage`/copy producer → `Indirect`; counters also read by shaders are part of that reachability. |

The concrete source has 80-byte camera, 48-byte material, 144-byte primary-frame,
80-byte post-settings and larger lighting records. Merely substituting root constants
for uniforms cannot work. WGSL uniform packing, storage packing and Slang packing
are not automatically identical: define offsets, matrix orientation, array stride,
bool encoding and padding once; validate Rust sizes/offsets and shader load results.
Preserve the existing material parameter reflection contract while changing its
transport. [Core bindings][core-layouts], [primary frame][primary-frame],
[lighting][lighting-layout], [post][post-shader].

Use an eight-word root convention, for example:

| Word | Meaning |
|---:|---|
| 0–1 | Frame-record buffer heap index, frame-record byte offset |
| 2–3 | Pass-record buffer heap index, pass-record byte offset |
| 4–5 | Draw-record table heap index, draw ID / base instance |
| 6–7 | View ID, pass flags or small dispatch-specific values |

This is a proposed renderer convention; simple kernels may use the eight words
directly. A tone record contains HDR/bloom/LUT sampled slots, regular sampler slot,
settings slice and output extent. GI/primary records point to larger scene, geometry,
instance and material tables; they do not add descriptor sets.

Persistent records live in owned device-local storage: `GeometryRecord` contains
vertex/index/tangent/previous-pose slices and strides; `InstanceRecord` contains
transforms, geometry/material IDs and application tags; `MaterialRecord` contains
texture-view/sampler indices and parameters. Descriptor slots remain immutable
through their last GPU use. Asset replacement publishes a new record generation
and retains the old resources until all referencing submissions complete.

Per-frame/pass/draw records occupy completion-gated upload pages. Host-write only
an unused/completed page; otherwise allocate another page or issue an ordered GPU
update. Cache immutable index tables; do not rebuild/upload every material index
for every pass. Track dirty rows. A byte offset replaces all four dynamic-offset
ABIs found in the source: VSM level/mip, VSM horizon/records, program lists, and glass
draw records (the latter hides behind `uniform(true, 16)`). Fixed sliced bindings,
such as shadow-view records, follow the same rule. [VSM][vsm-layouts],
[program lists][list-layouts], [glass records][tp-glass], [glass draws][tp-offsets].

A root pointing to a table is not a resource-use declaration. Each pass's resource
set is the union of direct resources and everything its frame/draw/material/geometry
records can reach. Reuse those CPU-side sets for barriers, residency and retirement.
For GPU-selected draw lists, declare the conservative union of eligible resources;
a CPU cannot discover the final subset by looking at an indirect counter.

## Generate barriers from a frame graph

Choose a frame graph rather than hand-written transitions in roughly sixty pass
families. Dynamic branches, resize/reload, retained caches, hidden table reads and
many intra-scope dispatches would duplicate state tracking across every pass.
Pass implementations declare logical access; one graph compiler owns ordering,
range state and native barrier lowering. Keep explicit edges for semantic side
effects not represented by resource reads/writes.

Ara's current `ara-render::graph` is a useful starting point, not a sufficient drop-in:
it accepts one access per whole resource per pass, and its transient descriptor is
simple 2D. Extract the scheduling module into a small `ara-framegraph` crate shared
by both renderer adapters, without making Renderlab depend on the old renderer's
lighting implementation. Add byte/mip/layer uses and texture-shaped allocation
before migrating HZB/VSM. This extraction is proposed work, not done by PR #5.
[Current graph](../../crates/ara-render/src/graph.rs),
[current usage index](../../crates/ara-render/src/graph/usage.rs).

Each node declares resource version, subresource range, read/write access, queue and
attachment load/store intent. Preserve source dispatch order first. In particular,
old compute scopes can contain dependent kernels; a graph node per old
`begin_compute_pass` is too coarse. Atmosphere LUT→sky→volume, GI queue phases,
VSM setup→indirect dispatch→mips, and skin vertices→clusters→AS all need internal
nodes/dependencies. [Atmosphere][at-encode], [GI][gi-encode], [VSM][vsm-encode],
[skinning][skin-encode].

Compilation should:

1. Reject undeclared/unknown resources, reads of undefined versions, cycles and
   incompatible simultaneous uses. Import actual last scheduled states and timeline
   dependencies, including retained history/cache contents from earlier frames.
2. Track the last writer and outstanding readers per overlapping range; emit RAW,
   WAW and WAR dependencies. Keep read→read free of barriers when owner/layout agree.
   A compute storage declaration can read and write, so equal `Storage` states still
   need ordering. Split intervals before comparing partially overlapping uses.
3. Batch compatible transitions at each dispatch/draw boundary with
   `Recorder::barrier_ranges`. Vulkan lowers exact ranges; Metal conservatively
   orders stages. Keep the same correctness model on both.
4. For queue changes, emit release/acquire plus timeline waits; same-family Vulkan
   queues still need execution dependencies although ownership transfer is omitted.
   Start all work on Graphics. Add async compute only after measurement proves a win.
5. Export final scheduled states and owning completion points for the next frame;
   compilation is not GPU completion. Never reset layout to Undefined just because
   a new frame begins. Retire upload pages, views and resources using completions,
   including Ara's two-deferred-frame admission limit versus Renderlab's three flights.

Example: `down[0]` writes image A; the next downsample reads A and writes B. Insert
`A: Storage → ShaderRead` and `B: Undefined → Storage`, then dispatch. After the
reverse up-chain, transition its final output from Storage to ShaderRead before
fragment tone mapping. For one-image mip pyramids, bind narrow per-mip sampled and
storage views so a sampled descriptor never spans mips currently in writable layout.

Other required chains are `FillBuffer → Storage → Indirect`, deformation Storage →
AsInput → BLAS build/refit → TLAS build → AsRead, and HDR Color/Storage → CopySource
→ per-glass copy → ShaderRead → next glass composite. History ping-pong is two
resource identities, not simultaneous read/write access to one image. Clear and
resolve operations declare writes too; in-pass resolve outputs use Color access.

Initially retain resources normally and avoid transient aliasing. Heap suballocation
with completion-gated retirement is already useful; aliasing overlapping resource
lifetimes is a different optimization. Cache compiled topology by settings/resource
generations, while patching frame records and actual imported states each frame.
Test the graph compiler as pure scheduling/range logic, then validate representative
native chains on Vulkan synchronization validation and Metal API/GPU validation.

## Pass mapping and difficulty

“Easy” means small binding/state surface, not permission to reduce the algorithm.
Material shading, decals and most reflections are composed into kernels; they are
migration work even when no separate pass name appears.

| Family / concrete work | Mapping onto ara-gpu | Difficulty / dependency |
|---|---|---|
| Bloom `downsample`, `upsample`; tone | Compute sampled→storage chains, then fullscreen raster with 3D LUT | Easy; first three migrations. [Post][post-encode], [tone][tone-shader] |
| `lc_build`, tile-max, motion blur, DOF | Storage records, R32Uint list image / RGBA16Float outputs | Easy–medium; next isolated kernels; preserve motion/camera-cut inputs. [Lights][lc-layout], [post][post-encode] |
| Atmosphere/environment | LUT, sky/ambient, volume/fog, environment array/mip kernels and sky draw | Medium; many dependent dispatches and optional shadow source. [Kernels][at-kernels], [encoding][at-encode] |
| Forward mesh, sky, conventional and local shadows | Explicit streams/instance steps, cull/depth/bias, depth array layers, 4x resolve | Medium; requires raster attachment subresource selection and converter draw-parameter verification below. [Forward][frame-forward], [shadow views][shadow-views] |
| Skinning | `skin_vertices → skin_clusters`, writable current/previous streams then BLAS refit | Medium; preserve pose history and AS build inputs. [Skinning][skin-encode] |
| Fresh receivers / `fresh_compute` | MRT receiver raster; texture/TLAS/light reads into R32Uint counts | Medium; geometry/scene tables and both ray/non-ray modes. [Fresh][fresh-encode] |
| Non-RT visibility | cull/finish, SW atomic raster, HW raster, HZB base/reduce, resolve/reset | Hard; u64 min/max in compute and fragment, indirect args, geometry arrays, mip hazards. [Visibility][vb-encode] |
| Primary RT, Interactive/repair | Inline query, hit/coverage/epsilon/receiver targets, repair queues and indirect dispatch | Hard; candidate and committed hit positions, mirrored/single-sided tests and full program ABI. [Primary][primary-encode], [ray handling][ray-shader] |
| Per-program deferred shading | Program bins/list dispatch, frame/draw/material index tables | Hard; preserves programmable surface/shade and per-program parameter reflection. [Lists][list-encode], [programs][program] |
| Virtual sun, surface cache, S4 | mark/compact/cull/finish/clear, SW/HW peel raster, mip chains, cached lookup/walk queues | Hardest synchronization slice; u64 atomics, four-byte offsets, array layers, indirect and persistent invalidation. [VSM][vsm-encode], [cache][surface-encode] |
| GI cache | probe/gather, claim/resolve loops, scan/setup/rays/finish/integrate/filter/light/cell shade | Hard; shared buffers change roles; retained cache state and RT/software ray backends. [GI][gi-encode] |
| Film / TR1 reconstruction | Accumulation and integer/float history ping-pong, `tr1_resolve` | Hard; previous-pose/identity/history reset equivalence matters more than dispatch syntax. [Film][film-encode], [TR1][tr-encode] |
| Glass/transmission/particles | Depth conversion/comparison, particle update/sort, copied HDR feedback, three-MRT lobes, reflection compute, ordered raster and MSAA | Hard; graphics/compute/copy hazards and dynamic per-glass records. [Transparency][tp-encode], [transmission][transmission] |
| Streaming/uploads/decals/reference tracer | Ordered staging, immutable views/records, descriptor retirement; reference tracing as a separate candidate module | Cross-cutting; preserve load cancellation, last-good reload and dirty updates. [Texture residency][residency], [reference][reference-modules] |

## Remaining gaps and awkward parts

The heap/32-byte-root model fits these workloads. The remaining issues are specific
interfaces, shader capabilities and graph machinery; restoring per-pass descriptor
sets or uniform buffers would not solve them.

| Issue found in the source audit | Required work before that migration slice |
|---|---|
| **Raster attachment mip/layer selection** | `BeginRenderingSubresources` now accepts `RenderAttachment` / `RenderTarget` for selected color/depth/resolve mips and slices. Metal clear/draw/resolve isolation tests pass, including nonzero layers/mips; the PC-owned Vulkan path still explicitly rejects this new command pending implementation. Renderlab's shadow/local-shadow constructors create individual depth-array layer views, which map directly to these targets. Do not split every shadow layer into unrelated images just to fit the current interface. [Shadow views][shadow-views], [local views][local-views], [current Attachment](../../crates/ara-gpu/src/commands.rs) |
| **Hit vertex return through the shader toolchain** | Installed Slang 2026.18.3 accepts `CommittedGetIntersectionTriangleVertexPositions` for SPIR-V but rejects DXIL at sm_6_6 and sm_6_10 with E36107 before MSC runs. Windows converter cooking alone does not implement it. Supply one Slang helper for candidate/committed positions: look up instance→geometry metadata, primitive's three indices and retained position slices, return object-space float3s in winding/barycentric order. Validate transformed/mirrored instances, nonzero ranges, refit and compaction against native query intersections. Optional native fetch can replace its implementation where proved. `HitGeometry::publish` and the shared `hit_geometry::SHADER` now implement this retained-buffer path; a native Metal query verifies exact known geometry with transformed instances, offsets and strides. Slang SPIR-V compilation/validation passes; Vulkan runtime and DXIL cooking remain unverified. [Slang target requirements][slang-position], [Renderlab uses][ray-shader], [GI uses][gi-ray-shader] |
| **Cooked draw parameters** | The Metal pipeline loader rejects converter stages needing draw-parameter binding. Native MSL stream tests do not certify all Slang `SV_VertexID`/`SV_InstanceID` combinations. Inspect fresh reflection and wire the converter contract or pass explicit draw/base-instance data where correct before porting instanced raster/shadows. [Loader](../../crates/ara-gpu/src/metal/pipeline.rs) |
| **Capacity versus old descriptor arrays** | Material arrays request 192 sampler entries, which can represent repeated descriptions. Dedup into 64 regular and 64 comparison states. Budget all live plus retired view generations against 128 storage-image slots and sampled/buffer capacities (Vulkan clamps). Count narrow mip/layer views separately; negotiate limits at renderer creation and diagnose exhaustion. Do not assume every lifetime fits from per-stage wgpu limits. [Requirements][required-limits], [material declarations][material-shader], [heap contract](../../crates/ara-gpu/AGENTS.md) |
| **Combined accesses** | Current `Access` is a small semantic enum, not arbitrary stage/access masks. Range-aware graph nodes may express more than one role. If a depth image is sampled while depth-tested read-only, or a buffer is simultaneously vertex input and shader storage, validate/lower that combination explicitly (including compatible view layouts); do not choose whichever enum happens to silence the validator. Separate nodes/images are valid only when they preserve the algorithm. [Access](../../crates/ara-gpu/src/lib.rs) |
| **64-bit atomics and sample types** | Capability bits do not prove the actual Slang→SPIR-V/DXIL→MSC compute and fragment atomic min/max variants, integer texture loads, multisampled depth reads or float filtering. Cook small probes and gate both physical devices before joining non-RT visibility/VSM. [Requirements][required-limits], [atomic raster][vb-atomic] |
| **WGSL authored programs** | Renderlab composes/reflection-checks WGSL `surface`, `shade` and `Parameters` through naga. Define the equivalent typed program interface in Slang, retain parameter diagnostics and last-good reload, and migrate built-in programs first. Keep existing WGSL programs on the wgpu runner until explicitly migrated; a Vulkan-only naga SPIR-V path is not a complete Metal solution. Preserve engine/user shader safety policy and do not claim old programs are source-compatible. [Program interface][program] |

## Incremental A/B and migration order

Sharing a physical GPU does not make a wgpu texture an ara-gpu image. Renderlab's
Vulkan development compiler already wraps one HAL device back into wgpu, but neither
project exposes the full shared creation/lifetime/queue/resource-state bridge needed
for arbitrary mixed pass encoding. Ara currently owns its native device. Metal
`adopt_texture` is useful for same-GPU Metal resources, not a generic wgpu ownership
transfer. Avoid sprinkling HAL imports and raw queue submits through pass code.
[Existing HAL wrapper][vulkan-import], [dedicated-device contract][lib-contract].

Use two levels of A/B:

1. **Per-pass replay during construction.** Freeze CPU scene/camera/settings/serial,
   input textures/buffers and expected outputs at a pass boundary. Upload independently
   to WgpuPass and AraPass runners. Choose either implementation in the replay harness;
   compare outputs and inspect diagnostic views. For dependent chains, the candidate's
   output feeds its next candidate node. Readback/upload is an acceptable development
   bridge; include its cost separately and never use it to claim layer speedups.
2. **Whole-frame backend A/B when a complete path exists.** Create either backend
   from the same CPU assets/settings and replay the same sequence, with equal history
   warm-up, camera cuts, cache invalidation, animation and streaming events. Keep the
   old backend switch until all supported modes pass. A comparison between wgpu Vulkan
   and ara Metal also changes GPU/platform; isolate layer effects on Vulkan separately.

This makes each pass move independently while the working renderer continues to
ship wgpu frames. It intentionally does not promise a production per-pass toggle
between two unshared devices. If live tail replacement becomes useful early, add
one explicit post-tail HDR transfer adapter with capability checks and ownership/
completion semantics; do not route HDR through an RGBA8 presentation surface or
force a native HAL shared-device redesign just for a temporary A/B switch.

| Slice | Move and exit evidence |
|---|---|
| 0. Foundation | Owned resource registry, storage-record ABI, frame graph, shader cooker/reload seam, capture/replay harness. Keep wgpu renderer unchanged as the oracle. Close remaining interfaces above only when their slice needs them. |
| **1. Bloom downsample** | Port `post.wgsl::downsample`, 8×8 groups, nine-tap tent, RGBA16Float output. Replay individual levels with odd extents, clamp edges, constant HDR and bright impulses; compare intermediate half-float pixels. |
| **2. Bloom upsample** | Port `post.wgsl::upsample`, reverse level order and exact 0.65 convex mix. Replay the complete dependent pyramid; check constant-energy preservation and boundary pixels. |
| **3. Post tone** | Port `post_tone.wgsl::{vs,fs}`, fullscreen triangle; HDR+bloom+3D LUT+80-byte settings record. Freeze grain serial; test LUT, aberration, grading and the actual output format/sRGB conversion. This creates a useful complete post-tail candidate. |
| 4. Complete screen tail and simple compute | Motion/depth extraction, tile-max, motion blur, DOF, `lc_build`, then atmosphere LUT/fog/environment chains. These build confidence in records, arrays/volumes and multi-dispatch state. |
| 5. Geometry/raster spine | Uploads/residency, materials/instances, forward sky/mesh, layered sun/local/transmission shadows, MSAA and skinning. Verify draw parameters and attachment subresources first. This is a complete forward candidate path, not yet RT/non-RT feature parity. |
| 6. Opaque visibility and lighting | Non-RT cull/SW+HW raster/HZB/resolve, fresh receivers; RT camera/Interactive after position-helper proof. Then per-program shading and mixed-program lists, preserving both paths. |
| 7. Retained lighting and history | VSM/cache/S4 and GI stages with exact invalidation semantics; film and TR1 with previous-pose and reset tests. Preserve the early-submit overlap once dependency traces match. |
| 8. Full parity and integration | Glass/particles/reflections and every remaining mode; full-frame A/B on game, PBR, quarry and Ara Showcase sequences. Only then switch Ara integration to its shared native device and retire the second-device output bridge. |

For every slice, source translation initially preserves math, workgroup size,
precision, integer overflow behavior and execution order. Compare meaningful
intermediate data before final images; hash exact integer queues/IDs/counters and
use stated float tolerances plus inspected output where compiler arithmetic differs.
Include resize, repeated reload failure/recovery, toggles, camera cuts, mirrored
instances, moving skinned meshes and multiple frames in flight. Report RT and non-RT
separately; a feature disabled for an isolated first fixture is not a shipped omission.

Performance acceptance uses quiet, validation-off paired runs with the same workload,
shader/compiler identities and declared transfer costs; correctness runs use native
validation. Account for upload, graph compilation, encoding, waits, driver pipeline
compile/reload and actual GPU pass times separately. Porting the layer does not by
itself establish faster GPU algorithms or cure the driver compilation stall.

## Source references

[at-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/atmosphere_gpu.rs#L1020-L1095
[at-inference]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/atmosphere_gpu.rs#L298-L330
[at-kernels]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/atmosphere_gpu.rs#L734-L824
[core-layouts]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L789-L905
[film-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/film_accumulation.rs#L594-L644
[frame-deferred]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L3211-L3295
[frame-early]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L3142-L3210
[frame-forward]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L3306-L3432
[frame-tail]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L3433-L3518
[fresh-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/fresh_shadow_gpu.rs#L523-L615
[gi-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/gi_cache.rs#L2709-L2865
[gi-ray-shader]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/gi_rays_hardware.wgsl#L1-L54
[lc-layout]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/light_cluster_gpu.rs#L236-L292
[lib-contract]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/lib.rs#L1-L9
[lighting-layout]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/lighting_gpu.rs#L73-L242
[list-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/program_lists.rs#L380-L412
[list-layouts]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/program_lists.rs#L31-L109
[local-views]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/light_cluster_gpu.rs#L58-L96
[material-shader]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/program_traced.wgsl#L1-L46
[modules]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/renderer.rs#L1-L58
[old-counts]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/docs/gpu-layer-bench.md#L28-L59
[post-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/post_gpu.rs#L736-L791
[post-shader]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post.wgsl#L1-L51
[primary-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/primary_ray_gpu.rs#L2202-L2509
[primary-frame]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/primary_ray_gpu.rs#L25-L46
[program]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/program.rs#L1-L108
[ray-shader]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/primary_trace.wgsl#L1-L155
[reference-modules]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/reference.rs#L813-L828
[required-limits]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/primary_ray.rs#L29-L79
[residency]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/texture_residency.rs
[shadow-views]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/shadow_gpu.rs#L190-L231
[skin-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/skinning.rs#L435-L510
[skin-layouts]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/skinning.rs#L249-L351
[slang-position]: https://docs.shader-slang.org/en/latest/external/core-module-reference/types/rayquery-03/committedgetintersectiontrianglevertexpositions-09cow12.html
[surface-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/virtual_shadow_cache.rs#L420-L488
[tone-shader]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post_tone.wgsl#L1-L51
[tp-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/transparency_gpu.rs#L1657-L2035
[tp-glass]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/transparency_gpu.rs#L567-L628
[tp-offsets]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/transparency_gpu.rs#L1920-L2006
[tr-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/temporal_reconstruction.rs#L1381-L1407
[transmission]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/transparency_shadow.rs#L287-L327
[vb-atomic]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/visibility_buffer_raster.wgsl#L1-L35
[vb-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/visibility_buffer.rs#L1380-L1492
[vsm-encode]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/virtual_shadow_gpu.rs#L2559-L2858
[vsm-layouts]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/virtual_shadow_gpu.rs#L731-L917
[vulkan-import]: https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/src/program_vulkan.rs#L182-L315
