# ara-render

NativeContent validates/packs on the worker; native_seed retains small GPU reuse metadata. Preallocate also polls native PSOs off-thread.
Publish in the ordered scope or destroy rejected candidates; the scene stays immutable until returned. Native/IBL pipelines survive content replacements.

Owns the M1 baseline renderer and agent visual feedback. Gameplay state stays in ECS; `Scene` is a render snapshot, not authoritative state.

## Public API

- `Graph`: declared accesses, stable topological ordering, barriers, explicit queue release/acquire pairs, timeline dependencies, transient images.
  Compilation indexes resource uses once; declaration-order versions and queue handoffs stay deterministic.
- `Renderer::new`, `render`, `render_view` (live debug view mode), `resize`, `capture`: cooked Vulkan rendering; read-only
  `gpu()` access for diagnostics/native presentation. Unchanged display resizes retain TAAU targets/history. `resize_upscaled(render, display)` is TAAU:
  scene passes, feature color/depth and IDs at `render_size()`; history, output, captures and picks at `size()`.
- `prewarm_view_quality` caches one camera on the startup worker; `create_view` claims it. `preallocate_view` reserves exact render/display targets, constants and retained pairs on the allocation worker; `submit_secondary` draws. Independent motion,
  exposure and TAA(U). Shared Prepare uploads and draws defer together; `poll_scene_frame` drains either camera.
  No primary render/readback. `render_secondary` waits for captures; Drop/release retire after pending use.
- `Scene`, `Mesh`, `Material`, `Light`, `CameraSpec`: extracted world data and camera pose/bounds requests. Materials use bindless texture indices.
- `instanced::Scene`: native model-space content contract for shared geometry,
  current/previous instance transforms, deforming revisions, 64-byte UV0/UV1
  vertices and 320-byte materials with six independent map transforms, AO,
  transmission and authored terrain layer IDs. Texture IDs address the native
  residency table; u32::MAX means neutral texels (separate from the legacy `Scene` ABI).
  Validate only at content publication; do not hash/traverse static vertices
  when moving instances. Culling bounds handle mirrored/nonuniform transforms.
- `Renderer::new_native` consumes this scene and an owned `instanced::Textures`
  provider on a raster-only device. `ara-texture-stream::NativeResidency` implements
  the provider without reversing its dependency on ara-render. Native fill uses
  compressed images, six independent UV slots, authored splats and local instancing.
  `update_native` updates transforms/skin poses; structural changes use
  `replace_native_scene`. Native draw lists cull and select cooked LODs, and the
  RG16F motion target carries unjittered current-minus-previous screen UV.
  `preallocate_native_textures` polls worker allocations against the last resident set;
  reused image ownership transfers at publication; rejected content keeps last-good.
- `update_native_changed(scene, instanced::Changes)` consumes exact instance,
  material and skin indices. Coherent writes/culling touch dirty groups only;
  idle frames retain visible lists and settle motion once; LOD bounds cache per pose. `native_frame_work`
  reports traversals/writes. `native_caster_changes` exposes old-union-new bounds;
  `cull_native_shadow(matrix)` caches opaque/masked casters for pass 6 using the
  light's texel density. Camera lists consume `Quality::lod_pixels/min_pixels`;
  shadows independently choose LODs within one texel and omit bound radii below
  max(2, min_pixels) texels. Explicit `*_with_resolution` shadow methods take the
  actual far-map/local-face viewport size (a cache key); camera LOD never dirties these shadow lists.
  Local shadows reserve `reserve_native_shadow_passes(count)` before graph imports;
  `cull_native_shadow_pass_with_resolution(slot, matrix, resolution)` returns a cached pass index,
  recorded as cascade 4+slot (its matrix comes from `look::local`'s table).
- `instanced::GeometryGpu` uploads static local vertices/LOD indices once. Changed
  deforming revisions use `prepare_pose` then `commit_pose`/`cancel_pose`; retire
  the old dependent TLAS before committing. Three skin slots retain presented and
  published vertices while a candidate is prepared. `presented` advances history
  only after a successful render. Raster pose barriers join the consuming graph;
  RT rebuilds changed skin BLASes (measured fallbacks, never claimed as refits).
- `update_geometry`: atomically replace meshes/materials/lights, retaining the
  device, targets and last good scene on failure. Textures/environment belong to
  asset integration. `entity_channel`, `entities`, `entity_at` expose picking.
- `update_materials` replaces parameters in their existing order and remaps scene texture slots without uploading
  geometry. `rename` updates an entity's capture label. Invalid edits retain the previous content.
- `submit_scene_frame` / `poll_scene_frame` keep one Scene frame in flight and two retained color/ID pairs; `output_size()` may lag a resize. UI reads only the completed pair.
  `entity_at` copies one texel on the UI queue; picks/outlines follow the displayed frame, while authored gizmo poses can lead by one Scene frame. `wait_scene_frame` is for capture/shutdown.
  Same-queue passes share a submission; cross-queue edges and deferred helper boundaries retain GPU waits/timestamps. Synchronous `render`/`render_view` remain available.
- `look::Look` (`Renderer::set_look`): D27 raster look after the G-buffer: physical sky LUTs and their IBL,
  CSM-shadowed froxel fog, histogram or manual exposure (snaps to its meter on a reset and while new GI arrives;
  `exposure`/`seed_exposure` carry a converged exposure across views), bloom, grading and the display curve,
  plus volume lens effects (`look::lens`): display lift/gamma/gain and chromatic aberration in the post pass,
  half-res gather DoF before TAA (TAA reads `DOF_COMPOSITE`), McGuire motion blur as the `look_post_motion`
  variant, AO/SSR tuning `OverPreset`. Their images allocate with the quality budget, never on a look edit.
  Hosts map `SceneLook` with `FrameEnvironment::raster` (ara-render-fidelity). The sky IBL,
  far map, cascades and horizon sun map rebuild on any sun change past `SUN_HOLD` (noise only): once
  something animates the sun, add a threshold with an old/new crossfade.
- `look::gi::ProbeSurfaces` (`Renderer::set_probe_surfaces`): probe GI over a `ProbeLayout` pool (up to four lattices, finest first, blended front to back by coverage so a fine lattice never seams into the next; `giPoolLobes`). One-time 8x8 oct surface caches;
  `GI.Relight` relights a budget per frame (sun via CSM, local lights, sky, emissive, last frame's probes) into 6x6
  irradiance, DDGI-style, without rays or noise; ping-pong atlases, so it copies only last frame's window forward (`GiGpu::relit`; cost follows the budget, not the pool). Capture resets relight all for `gi::CONVERGE_FRAMES` (exposure
  re-meters); live initial convergence uses bounded 4096-probe windows, then the preset budget. `preallocate_probe_volume` reserves/clears from `ProbeLayout` behind loading, including two staging slots; captures drain a partial clear. A new lattice fades in after convergence; `GI.Scatter` applies record uploads. A pool is ready on its coarsest lattice (`GiGpu::landed`): finer lattices may land later, converge at the preset budget and cross-fade in (`giParams.y`) without unsetting readiness.
- `look::Quality` (`Renderer::set_quality`): per-preset budgets from
  `ara_render_fidelity::presets::RasterPlan`: shadow cascades (count, texels,
  distance; stable light space, practical splits, gather PCF, cascade blend; one
  cached D32 map per cascade, redrawn only when its matrix or casters in its light volume change; moved casters
  patch only their texels (a per-list cell grid finds their casters, culling/patches.rs); outer cascades redraw whole at most one per frame, `Cascades::plan`), a cached far map (`look::far`),
  local-light shadows (`look::local`, patched alike; `in_gi_and_fog` also shadows GI relight and fog), half-res GTAO and SSR (cone footprint into
  last frame's colour pyramid), GI relight, bloom, emitter lights, LOD/cull pixels, terrain detail (zero binds `gbuffer_top`, no side projections), and the
  heightfield horizon (`look::horizon`: distant landforms' sun shadow, min'd into `visibility`, and sky visibility
  outside the probe volume), and sun virtual shadow maps (`look::vsm`, High/Epic: clipmap pages that pixels mark in `VSM.Mark`, read back once their frame
  completes, cached in world space, drawn under a per-frame budget; `VSM.Project` resolves them into a mask lighting reads, GI relight
  has its own pipeline, and no other shader inlines page lookups, which would cost every preset registers; moved casters re-dirty pages). Look images are allocated lazily; probe GI is gathered at half resolution.
- `set_deferred_post` + `finish(color, sampled, state)`: temporally resolve and grade
  an externally composited HDR frame (landscape, particles) with the same look post.
  TAA reprojects with native motion (Frame.look.w) when present.
- `CaptureRequest`, `Capture`: fixed tick/seed, settle frames, modes, PNG, picking,
  numbered marks, visible-pixel boxes, legends, content/compiler/driver provenance.
- `compare`: local RGB SSIM, explicit tolerance and an amplified diff image.

## Gotchas

The Vulkan SDK is required for cooking, not runtime. Cooked SPIR-V/DXIL and their reflection are committed under
`cooked/`; no shader compiler is linked at runtime. Use `../../shaders/pipelines.json`; changing the binding ABI requires a full cook.
The renderer rejects mixed source cohorts. Byte-address frame/material/light
records and 48-byte vertex records must match Slang's column-major ABI.

Shaders: `raster.slangh` is the shared ABI (bindings, Frame, Surface; Frame.look.x is the look record, look.w the
native motion texture, sizes/upscale the TAAU sizes, jitter offset and mip footprint; resolve and post use a
display-extent record). `look*.slang(h)` belong to the look; never use 16-bit intrinsics (f16tof32) there: the device
enables no Int16/Float16 capability, so pack halves with bit math.

Frames use reversed-Z, metres and linear HDR: sun in lux, point/spot flux in
lumens, environments in cd/m2, exposure a linear multiplier (README.md: tone mapper,
GPU numbers). Only the TAA history is RGBA16F and pre-exposed (`look.slangh`
exposureState); look passes learn from root word 7 whether input is history.
The G-buffer is compact: RGBA8 sRGB albedo + AO, RG16 octahedral normal, RGBA8
roughness/metal/AO occluder (zero for cutouts), B10G11R11 emissive, R32 ID and
RG16F motion; positions are rebuilt from depth. Shaders read and write it only through raster.slangh's
`gbuffer*` helpers, so the formats change there and in `Renderer::targets` together.
Local light lists are tiled 16x16 (`scene::MAX_LOCAL_LIGHTS` 256 per scene, the
first `TILE_LIGHTS` 128 per tile); `look::local` shadows the brightest few in view
(cube or spot faces, one cached D32 atlas). Spot lumens are point-equivalent, as in
the path tracer; `Light::parameters.z` softens the near field, `w` = 1 keeps it out of fog.

Capture determinism resets history, fixes host tick/seed, disables jitter/history
blending, locks exposure and renders the explicit settle count. No wall clock or
RNG enters a shader. Live TAA reprojects static extracted world positions and
rejects depth discontinuities; per-object animation velocities belong in M3.
Capture IDs are compact exact float render indices mapped to stable u64 entity
IDs. Persisted U32 render-ID channels use zero for background and metadata maps
indices to entities. Marks boxes cover visible pixels, including transparency.

Graph v0 allocates owned transient storage without aliasing. Imported resources
must declare their actual previous state; cross-queue imports need a release pass.
Static bindless reachability is conservatively declared for every graphics pass;
frame uploads use HostWrite imports. Raster culling and post share one graphics submission;
tiles stay on graphics between frames. Preparation can use compute. Readbacks are coherent
and read after completion. Static storage is device-local; startup staging uses its owning queue.

## Verification

Set `CARGO_BUILD_JOBS=2`. On Windows run `./run.ps1 -Mode Check`: it scopes Vulkan validation layers
and an empty implicit-layer directory to its process and restores the environment. SDL is built statically
by `ara-sdl3-src`; there is no SDL setup script, native directory or runtime DLL requirement.

Per-crate gates: `cargo check -p ara-render`, `cargo test -p ara-render`, `cargo clippy -p ara-render --all-targets -- -D warnings`, `cargo fmt -p ara-render -- --check`.
GPU goldens require this device/driver baseline with tolerance 1-SSIM <= 0.005.
They also check repeated deterministic captures, all 14 visible entities/picking,
and odd-resolution bounds framing. Update only deliberately with process-local
`ARA_UPDATE_GOLDENS=1 cargo test -p ara-render --test golden` (PowerShell: set the
environment variable before the command). Normal checks never update goldens.

`run.ps1 -Mode Capture` writes every debug view, marks, JSON and render IDs.
`run.ps1 -Mode Window` opens the same renderer through SDL; `--resize-test` on the windowed example exercises resize. `--frames=N` bounds its lifetime; Esc closes.
`cargo run -p ara-render --example timings` records 120 warm pass samples.

The glTF importer is example-only; production import belongs to the asset pipeline. Example media are CC0; source/licence notices accompany assets.
M3 owns material AST/fidelity, animation velocities, streaming/aliasing and richer lighting. Metal's native implementation is the cfg-gated ara-gpu stub.
