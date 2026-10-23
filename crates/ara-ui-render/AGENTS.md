# ara-ui-render

Purpose: persistent glyph atlases, ordered batches, Slang shaders and a working CPU reference adapter.
No document mutation, widget logic, layout, ECS, SDL or ara-gpu dependency belongs here.

Public API:
- `gpu::Renderer::{image,prepare,render,device_lost}` consumes retained display lists and font resources.
- `gpu::UiGpu::{upload,begin,draw,end}` is the complete host adapter contract.
- `gpu::{Instance,Batch,Clip,TextureUpload}` is the engine-neutral upload/draw ABI.
- `atlas::{GlyphAtlas,TextMode}` caches hinted raster glyphs or 48px three-channel outline distance fields.
- `CpuGpu` implements UiGpu with a tiny-skia premultiplied target; goldens exercise the same batches.
- `CpuRenderer::render` is an image-free one-shot PNG convenience facade.

ara-gpu/editor-shell adapter checklist:
1. Compile `shaders/ui.slang` offline with the checked-in `shaders/compile.ps1`.
2. Upload RGBA8 texture pages by opaque ID; copy upload bytes before returning.
3. Map `Instance` to a 64-byte, 16-aligned structured-buffer stride (four float4 records).
4. Bind instances at binding 0, bindless sampled images at 1, linear-clamp sampler at 2, set 0.
5. Supply the 32-byte push constants: first instance, texture index, mode, reserved, viewport, padding.
6. Draw six vertices per instance, preserving batch/painter order. Do not sort transparent batches.
7. Intersect rectangular clip ancestors into a scissor; rounded ancestors intersect stencil/masks.
8. UI output is premultiplied RGBA8 UNORM in sRGB numeric space, ONE / ONE_MINUS_SRC_ALPHA blending.
9. Linearize the completed UI texture before linear/HDR screen or world-surface composition.
10. The target/window owner supplies DPI, render-graph barriers, lifetimes and device recovery.
11. Call `device_lost` when attaching a fresh device/adapter; all image/atlas uploads will be replayed.
12. Read actual `batches` for batch-break inspection; display-list breaks are conservative estimates.

Fonts:
- Swash handles hinted masks, layered color outlines and CBDT/CBLC color bitmaps.
- Test font sources, hashes and OFL licenses are in `tests/fonts/`; corpus subsets are not shipping packs.
- COLRv1/SVG-only fonts are not an established rasterization format; use the tested CBDT pack for emoji.
- MSDF generation uses colored outline edges and nonzero winding, not a raster distance transform.
- Atlases are append-only for stable handles; host resource budgets may reset them between screens.

Verification (set `CARGO_BUILD_JOBS=4`):
- `cargo test -p ara-ui-render` compares all committed PNG pixels and behavior assertions.
- `cargo clippy -p ara-ui-render --all-targets -- -D warnings` and package rustfmt check.
- `cargo run -p ara-ui-render --example capture` emits PNG/structured inspection evidence.
- `cargo run --release -p ara-ui-render --example measure` records warmup + 5,000 raw samples per case.
- Set `ARA_UI_UPDATE_GOLDENS=1` only for an intentional rebaseline, inspect every changed PNG, then unset it.
- Measurements exclude offline CPU rasterization, PNG encoding and inspection JSON. GPU rate is unproven.
- Initial timing evidence is retained alongside final evidence; shared-machine scheduling outliers are included.

The host owns actual SDL IME, OS clipboard, AccessKit platform integration and GPU presentation.
