# ara-runtime-render

Presents `ara_runtime::extract::RenderFrame`s through native raster on every GPU
and preset (D27), with experimental fidelity as an explicit opt-in. Owns the
device and per-view upload state; no simulation, window or process code.

## Public API

- `Presenter::new(size, window_extensions, pipeline_cache_dir)`: the renderer is
  created on the first frame (headless when `window_extensions` is empty).
- `set_quality(QualityChoice)`: Auto, Low, Medium, High or Epic. The caller supplies
  the project default or user override. `set_render_options` selects experimental
  RT before device creation; otherwise no RT presentation resources are enabled.
  Rendering failures retain the completed image and expose `presentation_error`.
- `RasterView`: caller-owned renderer/device view shared by editor Scene and
  Presenter. `prepare` adopts published native generations, `render` draws native
  terrain/foliage and particles through the shared look. `capture` settles residency,
  probe acquisition and temporal history. It never discovers, cooks or imports.
  `render_secondary` draws an owned renderer camera over that content and shares
  model-arrival fades. Release follow-up: compose VFX in secondary Game previews
  too, with per-camera particle targets; particles currently use primary `render`.
- `RasterContent::new(frame, content)` prepares immutable CPU geometry/materials
  and the content key on a worker. `RasterView::publish_content` adopts it on the
  device thread, preserving last-good content if GPU publication fails.
- `RasterView::prepare_unchanged` is for a retained immutable extraction (editor
  accumulation/camera frames); it polls GI without revisiting root draws. Edits use
  `prepare` and its exact dirty rows. Any failed preparation discards that candidate
  CPU snapshot so retry cannot forget an accepted pose; GPU last-good stays live.
- `ModelPlaceholders` styles unpublished models from read-only import status:
  pending bounds are short grey corner marks (no filled faces or distant clutter);
  failed/missing stay solid magenta. Raster arrivals
  fade over 12 presented frames using material rows only; captures finish the fade.
- Probe GI is enabled by default, acquired asynchronously and disk-cached; live
  frames poll without waiting. `wait_probe_gi` is an explicit capture boundary.
- `prepare_with(&frame, resolver)` / `present_with`: native local geometry, compressed
  textures, instance poses and changed skin palettes. The resolver reads published
  terrain/material/prefab assets only on structural changes. Model-only callers can
  use `prepare` / `present` without a resolver.
- `output()` is a completed CopySource image independent of view targets; always
  pair it with `output_size()`. `size()` is the requested extent, which can differ
  while a failed replacement retains the last good output. The raster `renderer()`
  owns the shared device; its own output and size are not the selected presentation.
- `capture(&frame, size, mode, marks,
  seed)`: deterministic capture with numbered marks and a legend whose names are
  `Name (e_address)`; terrain and foliage use the same native G-buffer.
  `capture_presented(&frame, size, marks, seed)`: reads the GPU-composited window
  path, including landscape/foliage and persistent VFX (`presented: true`).
- Fidelity composes terrain, foliage and particles before shared HDR post/TAAU;
  native captures read the same view as windows and embedded Game. Raster puts
  terrain/foliage in the G-buffer, then particles before shared temporal/post.
- `stats()`: rebuilt flag, updated poses/skins, CPU prepare/render time and GPU ms.
  `host_wait_stats()` exposes cumulative host waits; snapshot around the whole
  windowed frame (including presentation), never subtract GPU timestamps from wall time.
- Raster look: `set_render_options(RenderOptions)` (D27: raster by default; RT opt-in),
  `set_look_override` (look development/scoring), `set_probe_gi(true)` +
  `probe_gi_pending()` / `wait_probe_gi()`: probe GI acquired per content key on a
  worker (one job at a time, newest request queued, full bakes disk-cached under the
  pipeline cache; delivered records apply 4096 per frame, scattered in-frame).
  The CPU bakes on half the cores (`gi_cpu.rs`: per-geometry SAH BVHs under an
  instance top level, map means via `NativeResidency::mean_texel` with the
  material's sRGB-decode bit for raw graph maps, terrain hits splat-weighted);
  `ARA_PROBE_GI_RT=1` opts into a temporary ray-tracing `VulkanDevice` + fidelity
  view, which contends with presentation for the driver.
  GI follows edits: `gi::Mobility` keeps instances that moved within 30 frames (and
  deforming geometry) out of the surfaces; when that set changes, or a new
  generation fits the lattice, the worker's persistent CPU tracer re-traces only
  nearby probes (`gi_cpu::affected`), which `Renderer::update_probe_surfaces` blends in.
  `tests/gi_edits.rs` checks a moved block's old spot against a fresh view.
  Emissive surfaces (`emitters.rs`, above 5 cd/m2) light the scene directly. A
  worker thread samples each emitter's triangles through its emissive map into
  0.75 m patches and casts rays against its own mesh: the flux escaping the
  fixture (shades, sockets) and its lobe become spot or point lights (two-sided
  only for open sheets, as in the path tracer), clustered until they fit
  `Quality::emitter_lights`. They follow their instances, ride after the scene's
  own lights and stay out of fog. Probe bakes zero those materials' emission.
  `stats().gpu: Option<GpuFrameCosts>` splits native scene updates, compressed
  texture uploads, VFX simulation/draw, raster/fidelity passes and publication.
  Residency counters accompany that same frame, including native Low. The legacy
  diagnostic path alone has incomplete coverage. Terrain/foliage are in view passes.
  Sum components per frame before quantiles; marginal pass medians do not add.
- `fidelity::scene(frame, Content)`: engine-owned scene conversion for fidelity.
  `Content` holds already-resolved terrain layers, foliage rule models, material
  detail and HDR environment overrides; it never reads sample locks or project paths.
  `fidelity::vfx::Effects` composites cooked particles into FidelityView HDR before post.
  This legacy capture adapter retains atlas packing; live presentation uses native.
- `fidelity::native::{Content::resolve, prepare}` resolves real Header.materials
  and model/prefab foliage at a control boundary, then prepares local shared geometry,
  six independent UV slots and native BC residency. It consumes existing runtime
  scatter/selector/material semantics; it does not scatter again, decode pixels or atlas.
  Prepared owns the CPU scene and its resident set for FidelityView::new_native.
  `Prepared::enable_motion` reserves independent root surface slots before upload;
  `update_frame` streams poses, factors and changed joint palettes without copying
  static vertices or asset handles. `FrameChange::Rebuild` requests fresh content
  for new identities/generations or terrain residency; keep presenting the last good
  view until replacement GPU publication succeeds.
- `fidelity::vfx::Effects::{update, composite_ticks}` maintains per-entity systems,
  world placement, scale and intensity, and preserves fidelity temporal AA after composition.
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
  (whose delta carries onto terrain). Square terrain extents; atlas tiles are relative
  to Header.min. Nonzero world origins require ara-landscape's CPU rebase support.
- `two_d`: 2D worlds (a `Camera2d`) draw through ara-render2d. `atlases(world,
  assets)` loads every referenced atlas generation (normal atlases linear);
  `Presenter2d::capture(world, atlases, tick, size, marks, seed)`.

## Gotchas

- `ara-render` takes glam 0.30 types (`glam30`); runtime math is glam 0.34.
- Skinning is CPU-side: GPU skinning needs a skinned vertex ABI in the cooked
  shaders (M3). Fox (1728 vertices) costs well under a millisecond.
- Native material/draw-set generations replace scene resources while retaining
  pipelines and targets. Light/transform/skin edits stream; native texture uploads
  never decode to RGBA8 or build an atlas, on any raster tier.
- At most 256 local lights (scene lights plus emitter lights; 128 per 16x16 tile);
  one directional sun (a dim default if the world has none).
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
