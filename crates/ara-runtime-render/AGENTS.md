# ara-runtime-render

Presents `ara_runtime::extract::RenderFrame`s with `ara-render` (Vulkan). Owns the
renderer and per-draw upload state; no simulation, window or process code.

## Public API

- `Presenter::new(size, window_extensions, pipeline_cache_dir)`: the renderer is
  created on the first frame (headless when `window_extensions` is empty).
- `prepare(&frame)`: rebuilds the uploaded scene only when the draw layout changes
  (entity, geometry, material); otherwise re-transforms moved/skinned meshes on the CPU
  (`Renderer::update_mesh`) and rewrites lights/sun in place.
- `present(&frame)`: `prepare` + render to the output image (blit with
  `ara_platform::Present`); resident terrain and foliage use ara-landscape on the
  same device. `capture(&frame, size, mode, marks,
  seed)`: deterministic capture with numbered marks and a legend whose names are
  `Name (e_address)`; terrain and foliage come from the landscape composite.
  `capture_presented(&frame, size, marks, seed)`: reads the GPU-composited window
  path, including landscape/foliage and persistent VFX (`presented: true`).
- Window presentation merges landscape and scene depth on the GPU before particles,
  then runs the shared HDR post pass. `output()` is the final CopySource image.
- `stats()`: rebuilt flag, meshes updated, CPU prepare time, render time.
- `fidelity::scene(frame, Content)`: engine-owned scene conversion for fidelity.
  `Content` holds already-resolved terrain layers, foliage rule models, material
  detail and HDR environment overrides; it never reads sample locks or project paths.
  `fidelity::vfx::Effects` composites cooked particles into FidelityView HDR before post.
  The current upload ABI still needs world-space/material merging and atlas packing;
  these are transitional until the descriptor and instancing migration lands.
- `fidelity::native::{Content::resolve, prepare}` resolves real Header.materials
  and model/prefab foliage at a control boundary, then prepares local shared geometry,
  six independent UV slots and native BC residency. It consumes existing runtime
  scatter/selector/material semantics; it does not scatter again, decode pixels or atlas.
  Prepared owns the CPU scene and its resident set for FidelityView::new_native.
- `geometry::{shape, transform}`: procedural shapes (box, sphere, capsule, plane;
  centred, planes at y = 0) and world-space transform with 8-influence linear-blend
  CPU skinning. `sky(env)`: 64×32 gradient environment. `decode_texture`: BC7 KTX2
  → RGBA8 (the renderer uploads RGBA8 today).
- `camera(&frame)`: the frame camera as a `CameraSpec` (overview fallback).
- `vfx::Effects`: GPU particles in captures. Each `VisualEffect` replays its cooked
  library effect (ara-vfx-gpu `cooked/<name>`, at most 240 ticks) over the frame the
  renderer just captured (HDR background, reversed-Z depth), camera shifted into the
  effect's space; `Presenter::capture` composites before resizing back.
- `landscape::Landscapes`: ara-landscape (CDLOD terrain + scattered foliage) rendered
  from the capture camera with the scene projection and merged by depth, after VFX
  (whose delta carries onto terrain). Square terrain extents; atlas tiles are relative to Header.min. Nonzero world
  origins require ara-landscape's CPU rebase support.
- `two_d`: 2D worlds (a `Camera2d`) draw through ara-render2d. `atlases(world,
  assets)` loads every referenced atlas generation (normal atlases linear);
  `Presenter2d::capture(world, atlases, tick, size, marks, seed)`.

## Gotchas

- `ara-render` takes glam 0.30 types (`glam30`); runtime math is glam 0.34.
- Skinning is CPU-side: GPU skinning needs a skinned vertex ABI in the cooked
  shaders (M3). Fox (1728 vertices) costs well under a millisecond.
- Material or draw-set changes rebuild (`replace_scene`, keeps device/pipelines);
  light and transform edits stream. Model texture decodes are cached per generation.
- At most 64 local lights; one directional sun (a dim default if the world has none).
- Capture resizes the renderer to the request and back.
- `Presenter2d::set_atlases` publishes textures once; `prepare` moves the atlas map
  through extraction without copying pixels; `present` draws without readbacks.
- Cook the compositor offline: `cargo run -p ara-shader-cook --
  crates/ara-runtime-render/shaders/pipelines.json crates/ara-runtime-render/cooked`.
- Effects come from ara-vfx-gpu's committed cooked library (a dev-checkout path);
  authored project effects need an asset-pipeline cook first.

## Testing

GPU tests need the Vulkan SDK layers: `source .sn/env.sh` (bash) or
`../ara-render/run.ps1` style process-local `VK_IMPLICIT_LAYER_PATH`/`VK_LAYER_PATH`.
`RUST_TEST_THREADS=1 cargo test -p ara-runtime-render`: `tests/headless.rs` renders
the demo scene, walks the Fox, checks streaming updates, a live light edit and that
validation (including synchronization validation) stays clean. Captures land in
`.sn/runtime-render/`. `cargo clippy -p ara-runtime-render --all-targets -- -D warnings`.
