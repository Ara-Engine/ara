# ara-render2d

Owned Vulkan 2D render path on ara-gpu, reusing ara-render's graph, ToneMap and capture.

## Public API

- `Scene2d::extract(world, atlases, tick)`: immutable extraction from a fixed-tick ECS
  snapshot. Sprites, flipbooks (evaluated at `tick`), tilemaps, lights, shadow outlines.
  `PhysicsPose2d` wins over the Transform hierarchy for physics entities.
- `AtlasImage::from_cooked`: validates an AR2A v1 payload (no native struct casts).
- `Renderer2d::new(scene, size, cache)` uploads atlases and the extraction (stage it
  off-frame); `update(scene)` replaces the extraction between frames on the same device
  (buffers grow; atlas set must match); `render(camera, exposure)` writes constants only;
  `capture(camera, request)` returns the shared capture contract (PNG, entity IDs,
  picking, marks, provenance hashed lazily from resident content). Views: lit,
  albedo, normals, entity_ids, shadow_cascades (2D shadow visibility).
- `with_extensions(scene, size, cache, extensions)` adds window support without
  changing `new`. `present(camera, exposure)` skips capture readbacks and leaves
  `output()` in CopySource state; `size()` lets window owners stage resized targets.

## Rules

- Scene2d is never authoritative gameplay state; stage new renderers off the human's
  frame and swap at frame boundaries (D20). `update` validates everything before any
  GPU write, so a rejected update leaves the previous frame resident.
- `batch::prepare` is per-frame code: borrow from the extraction, no per-sprite
  allocation (100k sprites: 13 ms dev profile).
- Sprites: procedural instancing (6 vertices; nine-slice 54), bindless atlases, stable
  painter sort (sorting layer, order, identity). Batches never merge across an
  intervening sort key. Tile layers become batches culled with parallax- and
  pivot-aware bounds: 32x32 cell chunks, or, when a map has tiles larger than their
  cell, world-space depth bands drawn far to near (any layout or basis), each sorted
  back to front.
- Pixel mode snaps camera and pivots to art pixels, samples nearest, never jitters.
- Color atlases are SRGB, normal atlases UNORM. Lighting and shadows run only for
  opt-in lit sprites/tiles; unlit draws skip the light loop entirely.
- Shadows test `tilemap::outline` segments: counter-clockwise, front-facing only, so
  solid tiles are lit through their own surface. Keep winding when mirroring.
- Cook shaders offline (`run.ps1 -Mode Cook`); never compile at runtime or in build.rs.
  Delete the superseded `cooked/<hash>` directory after a recook.

## Sample and goldens

- `sample/platformer.commands.json` is the source of truth: Engine Protocol batches
  that author the whole scene. `platformer prepare` replays it into
  `platformer.scene` (seeded IDs); a test fails if they drift.
- `examples/common::scene` projects the scene, runs 120 physics ticks, then extracts.
- Goldens need this PC's Vulkan SDK. `ARA_UPDATE_2D_GOLDENS=1` rewrites them; always
  look at the images before committing.

## Test

Set CARGO_BUILD_JOBS=2. `./run.ps1` (-Mode Check|Cook|Prepare|Capture|Benchmark).
Check runs test, clippy (--all-targets -D warnings) and fmt --check for the 2D crates.
