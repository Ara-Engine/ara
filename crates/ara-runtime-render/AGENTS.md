# ara-runtime-render

Presents `ara_runtime::extract::RenderFrame`s through the renderer installed on a
`Presenter` (Renderlab, `ara-renderlab`, the only renderer) and resolves what a frame needs
to be drawn: models, materials, terrain, foliage, decals, the look and the camera. It owns
the presentation device and the completed output; no simulation, window or process code.

## Public API
- `Presenter::new(size, window_extensions, pipeline_cache_dir)` / `with_device`: the first
  `prepare`/`present` opens a `PresentationDevice` (headless when `window_extensions` is
  empty). `gpu()` / `gpu_mut()` give the device (None before it opens).
- `set_external_renderer(Box<dyn ExternalRenderer>)`: presents, captures and `live_image` draw
  the frame with that renderer and publish its image as `output()`. Without one a present
  fails ("no renderer is installed"). `ExternalView` carries the frame, the look, the content's
  environment map, foliage parts, terrain materials, the foliage share and terrain detail
  distance; `ExternalFrame` returns the image, optional packed ids, GPU ms and pass times.
  Presents are synchronous: nothing is in flight, so `output()` is complete when a present returns.
- `present_with(&frame, resolver)` / `present`: the resolver (`fidelity::native::Content`)
  runs when the draw count changes. A failed present keeps the last published image and
  exposes `presentation_error`. `stats()` has CPU prepare/render time, GPU ms, the renderer's
  handoff time and `gpu: Option<GpuFrameCosts>`; `pass_timings()` lists the renderer's own passes.
- `set_quality(QualityChoice)`, `set_render_options`, `set_graphics_settings`,
  `graphics::plan`: the user's tier selects what the renderer is told (foliage share, terrain
  detail, the look's bloom/blur/DOF switches). `quality()` is the preset the device resolves to.
- `set_debug(RenderDebug)`: `exposure` locks EV100 (`environment_locked`) and `view` picks the
  debug view captures draw; any other switch is reported to the renderer as `debug_switches`.
  `reference` is refused (the path-traced reference is gone). `set_look_override` replaces the look.
- `capture(&frame, size, mode, marks, seed)` / `capture_with`: the renderer's image, settled
  and read back; a debug view other than Lit fails `ARA_VIEW_MODE_UNAVAILABLE`.
- `PresentationImage::publish`: a copy of a completed image the compositor can sample while
  the next frame renders; `readiness()` is renderer, published, rebuilt, failed.
- `environment_at(frame, eye)`, `camera`, `sky`, `decode_texture`, `mean_texel`: the look at an
  eye, the frame's camera, the sky gradient and texture decoding the bridge and editor share.
- `geometry::{shape, transform}`: procedural shapes (box, sphere, capsule, plane) and
  world-space transform with 8-influence CPU skinning. `terrain_mesh`: a terrain tile as a mesh.
- `ModelPlaceholders` styles unpublished models from read-only import status: pending
  bounds are short grey corner marks; failed/missing stay solid magenta.
- `fidelity::native::{Content::resolve, prepare}` resolves real Header.materials and
  model/prefab foliage at a control boundary, then prepares local shared geometry, six
  independent UV slots and native BC residency. It consumes existing runtime
  scatter/selector/material semantics; it does not scatter again, decode pixels or atlas.
  `Prepared` owns the CPU scene and resident set; `update_frame` streams poses, factors and
  joint palettes without copying static vertices; `FrameChange::Rebuild` requests fresh content.
  Vertex colours map into `Vertex::color`; `Draw.paint` becomes `Instance::paint`; graph paint
  layers become layer material rows; decals (`fidelity/native/decals.rs`) become native
  decals plus one material row.
- `two_d`: 2D worlds (a `Camera2d`) draw through ara-render2d (`Presenter2d`). `atlases(world,
  assets)` loads every referenced atlas generation; `capture(world, atlases, tick, size, marks, seed)`.

## Gotchas
- Renderlab's bridge reads `Content` (environment map, foliage parts, terrain materials) and
  `terrain_mesh`; keep their shapes stable or change `ara-renderlab` with them.
- Skinning is CPU-side here.
- `PresentStats.gpu_span_ms` measures the device span of the last work; `gpu_ms` is the
  renderer's own figure plus publication, not a whole-frame gate.
- `fidelity::native` needs `ara-texture-stream`'s native sampling source to match.

## Testing
`cargo test -p ara-runtime-render` (content resolution, readiness, graphics plans, geometry);
`cargo clippy -p ara-runtime-render --all-targets -- -D warnings`. Renderer-level image tests
live in `ara-renderlab` (`tests/render.rs`).
