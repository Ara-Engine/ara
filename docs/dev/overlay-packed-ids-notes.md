# Overlay packed ids: handover notes (work in progress, Mac only so far)

Goal: outline, hover and grid occlusion read entity ids through one source abstraction, with
two sources: ara-render's float id channel (as before) and a PACKED RGBA8 image (Renderlab's).

## What exists on this branch

- `crates/ara-editor-gpu/shaders/ui_body.slang`: one `idAt(slot, texel, packed)` fetch used by
  outline, hover (mode 5) and the grid's occlusion (mode 7). Packed = RGBA8 UNORM, 32-bit id
  little-endian across R,G,B,A; 0 = nothing; `0xFFFFFFFF` = a hit with no id (hides the grid,
  never outlines: `outlineKey` maps it to 0). Flag bit 20 of an outline/hover draw's
  `parameters.w` (the grid's axes record `parameters.x` = 1) marks a packed texture.
  New mode 12 = the debug repack (float id texel to packed texel, written with no blend over a
  cleared target). Recooked: `cooked/d4b4…` (Metal, DXIL, SPIR-V via `ara-shader-cook`) and
  `shaders/fragmentMain.spv` (slangc with compile.ps1's arguments). `vertexMain.spv` is unchanged.
- `ara-editor-gpu` Metal adapter: `IdEncoding {Float, Packed}`, `NativeUi::set_id_encoding`,
  `NativeUi::repack_ids(Some(external_id))` (a pre-pass in `submit()` repacks the float ids into
  its own RGBA8 target the overlays then read), `NO_ID_HIT`.
- Editor: `studio/ids.rs` (`IdSource {Renderer, Packed, Repacked}`, `ARA_PACKED_IDS=1` selects
  `Repacked`), `compositor.rs` split into `compose` + present, `IdSource::apply`, hover overlay
  (`Overlay::Hover`) emitted for non-`Renderer` sources from `Local::scene_pointer`, outlines
  drawn as overlays for non-`Renderer` sources (`in_scene = draws_outlines && renderer source`).

## Findings (for the Renderlab hand-off)

- Overlays need ids only, not depth. Gizmo lines/quads and icons (modes 6, 8, 2) read neither;
  the grid hides wherever id != 0 (with depth it could show over geometry behind its plane).
- A raster Scene on ara-render draws the selection outlines itself (`set_outlines`); the overlay
  outline path (mode 5) is only used when the Scene does not. The studio never emitted
  `Overlay::Hover` before this branch.
- The `window` capture / `--screenshot` do not include the compositor's overlays (grid, gizmo,
  outlines): they are the Scene capture plus the UI snapshot. Use `screencapture` of a live window.

## Verified

- Mac, real window: `ARA_PACKED_IDS=1` with two selected entities draws the same outlines as the
  default (side by side crops looked identical). Not yet pixel-compared.

## Not done

- The pixel-for-pixel test of the packed path against the direct one (outline, hover, grid):
  plan is a Mac GPU test in `studio/ids.rs` that composes a frame twice through
  `Compositor::compose` (renderer source with outlines forced to overlays, then `Repacked`) and
  compares `ui.pixels()`.
- The Vulkan adapter (`src/vulkan/`): it has none of `IdEncoding`, `set_id_encoding`,
  `repack_ids`, nor the packed flag, so the editor does not build on Windows/Linux from this
  branch until they are ported (same shape as `src/metal/`; submit.rs is the larger job).
- Picking (`Compositor::pick` uses `renderer.entity_at`) still reads ara-render.
