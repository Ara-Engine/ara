# Ara 2D toolset integration

The four crates (`ara-2d`, `ara-import2d`, `ara-physics2d`, `ara-render2d`) are picked
up by the root workspace wildcard; no root manifest change is needed. Protocol/editor
routing stays with the host.

## Host wiring

- Schemas and commands: `ara_2d::commands::register` and
  `ara_physics2d::commands::register` on the shared SchemaDb/Registry (they also call
  the plain `register`). Diagnostics: ARA3601 (2D authoring), ARA3602 (physics).
- Simulation: `ara_2d::register_sim` and `ara_physics2d::register_sim` before building
  EditWorld/Session, then `ara_physics2d::install(session, config)` after gameplay
  input systems. Pause the session when `Physics2dStatus::error` is set.
- Assets: install `ara_import2d::{AtlasImporter, TilemapImporter}` in an ara-asset
  worker registry, never on the editor frame. Dependency PNGs arrive as immutable
  `ImportInput` snapshots. Canonical atlas metadata has no GPU slots.
- Rendering: `Scene2d::extract` from a fixed-tick snapshot plus boundary-published
  atlases, then `Renderer2d::new` off the human's frame; swap at frame boundaries
  (D20). Ordinary camera frames update constants only.
- Capture: route protocol captures here when the view selects a `Camera2d`. The
  daemon router is not changed by these crates.

## Authoring model

Every 2D asset and scene is canonical text. A tilemap's text holds its source cells
(one short line per cell); AR2C 32x32 chunk payloads are disposable cache objects.
`sample/platformer.commands.json` shows the whole agent surface: it is a list of
Engine Protocol batches (`tilemap.set`, `tilemap.fill` with `autotile`,
`tilemap.paint`, `tilemap.erase`, `tilemap.colliders.generate`, `sprite.set`,
`flipbook.set`, `physics2d.set`, `light2d.set`, `camera2d.set`) that produces
`platformer.scene` exactly. Each batch is one undo item; a pointer stroke is one batch.

## Rendering

Orthographic `Camera2d` in art pixels per metre with integer zoom and nearest
sampling in pixel mode. Bindless atlases, procedural instancing (6 vertices, nine-slice
54), stable painter sort. Tile layers become immutable 32x32 batches culled per chunk
with parallax-aware bounds; layers have parallax and tint. Normal-mapped lights and
outline shadows are opt-in per sprite or tile definition; unlit draws skip the light
loop. Post reuses the main renderer's exposure/ACES/sRGB `ToneMap`; pixel art never
jitters. The resident atlas count is bounded by the common Vulkan heap; overflow is a
structured GPU error.

Additive shared hooks (minimal): `Command::DrawProcedural` in ara-gpu; in ara-render
`CameraSpec::Orthographic`, public `CaptureContext`/`Capture::from_render`, and
`renderer::post::ToneMap`.

## Measurements (RTX 4070 SUPER, 1280x720, Vulkan timestamps)

| Workload | Sprite pass p50 / p95 |
|---|---|
| 100k unlit sprites, 2.5 px | 0.124 / 0.133 ms |
| 100k unlit sprites, 14 px (~21x overdraw) | 1.43 / 1.59 ms |
| Platformer sample, 3 shadowed lights, 22 edges | 0.081 / 0.083 ms |

Shared ToneMap is 0.03 ms; capture readback 0.28 ms is diagnostic-only. 8 warm-up +
120 measured frames, immutable resident instances, one frame in flight; CPU extraction
and import are not included. Data: `measurements/100k-sprites.json` (the previous
build's single-workload run is kept as `100k-sprites-before-outline-shadows.json`).
Reproduce with `./run.ps1 -Mode Benchmark`.

## Running

`./run.ps1` sets CARGO_BUILD_JOBS=2 and the Vulkan SDK `Bin` on PATH. Modes: Check
(test, clippy -D warnings, fmt), Cook (after shader edits), Prepare (replay the
command script into the sample sources), Capture, Benchmark. Goldens need this PC's
Vulkan SDK; `ARA_UPDATE_2D_GOLDENS=1` deliberately rewrites them.
