# ara-render

Owns the M1 baseline renderer and agent visual feedback. Gameplay state stays in
ECS; `Scene` is an immutable render extraction snapshot, not authoritative state.

## Public API

- `Graph`: declared resource accesses, stable topological ordering, barriers,
  explicit queue release/acquire pairs, timeline dependencies, transient images.
- `Renderer::new`, `render`, `render_view` (live debug view mode), `resize`, `capture`: cooked Vulkan rendering; read-only
  `gpu()` access for diagnostics/native presentation. Resize resets history.
- `Scene`, `Mesh`, `Material`, `Light`, `CameraSpec`: extracted world data and
  camera pose/bounds requests. Materials use bindless texture indices.
- `instanced::Scene`: native model-space content contract for shared geometry,
  current/previous instance transforms, deforming revisions, 64-byte UV0/UV1
  vertices and 320-byte materials with six independent map transforms, AO,
  transmission and authored terrain layer IDs. Texture IDs address the native
  residency table; u32::MAX means neutral texels. This input is separate from
  the legacy `Scene` ABI.
  Validate only at content publication; do not hash/traverse static vertices
  when moving instances. Culling bounds handle mirrored/nonuniform transforms.
- `Renderer::new_native` consumes this scene and an owned `instanced::Textures`
  provider on a raster-only device. `ara-texture-stream::NativeResidency` implements
  the provider without reversing its dependency on ara-render. Native fill uses
  compressed images, six independent UV slots, authored splats and local instancing.
  `update_native` updates transforms/skin poses; structural changes use
  `replace_native_scene`. Native draw lists cull and select cooked LODs, and the
  RG16F motion target carries unjittered current-minus-previous screen UV.
- `instanced::GeometryGpu` uploads static local vertices/LOD indices once. Changed
  deforming revisions use `prepare_pose` then `commit_pose`/`cancel_pose`; retire
  the old dependent TLAS before committing. Three skin slots retain presented and
  published vertices while a candidate is prepared. `presented` advances history
  only after a successful render. Current AS API rebuilds changed skin BLASes;
  these are measured rebuild fallbacks, never claimed as native refits.
- `update_geometry`: atomically replace meshes/materials/lights, retaining the
  device, targets and last good scene on failure. Textures/environment belong to
  asset integration. `entity_channel`, `entities`, `entity_at` expose picking.
- `update_materials` replaces parameters in their existing order and remaps scene
  texture slots without uploading geometry. `rename` updates an entity's capture
  label. Invalid edits retain the previous content.
- Live frames keep output/IDs on the GPU. Captures copy the final settled frame
  once; `entity_at(&mut self, pixel)` reads IDs only when explicitly picking.
- `look::Look` (`Renderer::set_look`): D27 raster look after the G-buffer:
  physical sky LUTs and their IBL, CSM-shadowed froxel fog, histogram or manual
  exposure, bloom, grading and the display curve. Defaults reproduce M1. Hosts map
  `SceneLook` with `FrameEnvironment::raster` (ara-render-fidelity).
- `look::gi::ProbeSurfaces` (`Renderer::set_probe_surfaces`): probe GI. A one-time
  acquisition (`FidelityView::bake_probe_surfaces` on RT devices) records 8x8 oct
  surface caches; each frame `GI.Relight` relights them (sun via CSM, local lights,
  sky, emissive, last frame's probes) into 6x6 irradiance, sampled DDGI-style.
  No runtime rays, no per-pixel noise.
- `set_deferred_post` + `finish(color, sampled, state)`: grade an externally
  composited HDR frame (landscape, particles) with the same look post.
- `CaptureRequest`, `Capture`: fixed tick/seed, settle frames, modes, PNG, picking,
  numbered marks, visible-pixel boxes, legends, content/compiler/driver provenance.
- `compare`: local RGB SSIM, explicit tolerance and an amplified diff image.

## Gotchas

The Vulkan SDK is required for cooking, not runtime. Cooked SPIR-V/DXIL and their
reflection are committed under `cooked/`; no shader compiler is linked at runtime.
Use `../../shaders/pipelines.json`; changing the binding ABI requires a full cook.
The renderer rejects mixed source cohorts. Byte-address frame/material/light
records and 48-byte vertex records must match Slang's column-major ABI.

Shaders: `raster.slangh` is the shared ABI (bindings, Frame, Surface; Frame.look.x
is the look record, look.w the native motion texture). `look*.slang(h)` belong to
the look; never use 16-bit intrinsics (f16tof32) there: the device enables no
Int16/Float16 capability, so pack halves with bit math.

Frames use reversed-Z, metres and linear HDR. Sun input is lux, point/spot flux
is lumens and HDR environment input is radiance in cd/m?. Exposure is a positive
linear multiplier. See README.md for the exact tone mapper and GPU measurements.
Local light lists are tiled 16x16, with a checked scene capacity of 64 lights.

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
frame uploads use HostWrite imports. Frame end returns light tiles to compute ownership. Readbacks stay coherent and
are read only after timeline completion. V0 serializes frames for inspection. Static scene storage is device-local;
startup staging transfers run on the eventual owning queue.

## Verification

Set `CARGO_BUILD_JOBS=2`. On Windows run `./run.ps1 -Mode Check`.
It scopes Vulkan validation layers and an empty implicit-layer directory to its
process and restores the environment. SDL is built statically by `ara-sdl3-src`;
there is no SDL setup script, native directory or runtime DLL requirement.

Per-crate gates: `cargo check -p ara-render`, `cargo test -p ara-render`,
`cargo clippy -p ara-render --all-targets -- -D warnings`,
`cargo fmt -p ara-render -- --check`.
GPU goldens require this device/driver baseline with tolerance 1-SSIM <= 0.005.
They also check repeated deterministic captures, all 14 visible entities/picking,
and odd-resolution bounds framing. Update only deliberately with process-local
`ARA_UPDATE_GOLDENS=1 cargo test -p ara-render --test golden` (PowerShell: set the
environment variable before the command). Normal checks never update goldens.

`run.ps1 -Mode Capture` writes every debug view, marks, JSON and render IDs.
`run.ps1 -Mode Window` opens the same renderer through SDL; `--resize-test` on the
windowed example exercises resize. `--frames=N` bounds its lifetime; Esc closes.
`cargo run -p ara-render --example timings` records 120 warm pass samples.

The glTF importer is example-only; production asset import belongs to the asset
pipeline. Example media are CC0; source/licence notices are next to the assets.
M3 owns material AST/fidelity, animation velocities, streaming/aliasing and richer
lighting. Metal's native implementation is the cfg-gated ara-gpu backend stub.
