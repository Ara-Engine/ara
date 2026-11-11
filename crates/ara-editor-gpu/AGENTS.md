# ara-editor-gpu

Editor-only UiGpu adapter: Vulkan (`src/vulkan/`) over the renderer's ara-gpu device
everywhere except macOS, where `src/metal/` draws through the portable ara-gpu API on
a MetalDevice it owns (same GPU; renderer images and Play slots adopted by texture).
No project, scene-authority, widget or input state belongs here.

Backend-neutral calls for hosts (editor, splash, Hub): `UiDevice` (Vulkan context, a
marker on Metal), `ui_device(&GpuDevice)`, `open_presentation`, and
`NativeUi::{create_present, present, resize_present, retire_present, wait_idle}` over
ara-platform (`resize_present` takes a live-resize reach, `Present::resize`). Hosts call
`pin_target_size(drawable)` before each render: the integer logical size times a
fractional dpi can miss the drawable by a pixel, and presenting would rescale the UI.

API: `NativeUi`, `Overlay`. The constructor/native image import are unsafe:
the caller retains the creating renderer/device until this adapter is dropped.
All queue access is serialized on the window thread. End waits for completion;
the completed RGBA8 UNORM target is CopySource for ara-platform presentation.

The shared instance ABI is exactly 64 bytes, aligned to 16, with no padding.
Bindings: 0 instances, 1 fixed 256 sampled images, 2 immutable linear-clamp sampler,
3 rounded clip masks. Push constants are 32 bytes. Preserve painter order.
Rounded ancestor masks intersect in the fragment shader; rectangular bounds also
intersect the physical-pixel scissor. Raster and MSDF atlases use the same uploads.
UI output is premultiplied sRGB numeric RGBA8, ONE/ONE_MINUS_SRC_ALPHA blending.
Scene output already contains the renderer's SDR tonemap. No CPU viewport copy.
External images enter and leave CopySource; never retain a destroyed resize view.
Resources clean partial construction and are destroyed after queue completion.

Cook offline with `shaders/compile.ps1`; checked-in SPIR-V is runtime input.
The shader logic is `shaders/ui_body.slang`: `ui.slang` (Vulkan layout above) and
`ui_bindless.slang` (engine bindless ABI: root words instances/masks slots, first,
texture|mode<<16, maskOffset|count<<24, clip origin/size u16 pairs, dpi; the draw's
viewport is its clip) include it. Metal needs the latter cooked on the canonical host:
`ara-shader-cook crates/ara-editor-gpu/shaders/pipelines.json crates/ara-editor-gpu/cooked`
with Metal Shader Converter installed. Keep `ui.slang`'s SPIR-V byte-identical when
refactoring the body (compile both entries before and after with the same slangc).
Do not enable runtime descriptor arrays: ara-gpu's device uses a fixed-size heap.
The pixel contract is `tests/ui_pixels.rs`: actual GPU output against CpuGpu for
raster/MSDF, nested clips and alpha, windowless on either backend (local SSIM loss <= 0.01).
The old editor shell's `--verify` route also ran it over complete editor screenshots in both
semantic themes; that route went with the shell (#53).

Set CARGO_BUILD_JOBS=2. Run cargo check/clippy/test -p ara-editor-gpu, with process-local
empty VK_IMPLICIT_LAYER_PATH.

Scene overlays are inserted immediately after the viewport image, before later
UI controls/popovers. Grid and hover execute on the GPU against the scene ID map.
The grid uses derivatives for antialiasing and distance fade, with theme axis tints.
The editor draws mode-5 outlines only when the Scene is not raster (a raster Scene draws its own: ara-render `Outlines`). Outlines and hover (mode 5) group channels through an optional table
(`outline_groups`): a group outlines as one shape; ground (terrain) only against sky,
void and other ground; edges are a 1 logical px antialiased band each side (distance to the
nearest ID texel across the edge). `Icon` draws an uploaded white image tinted (mode 2). Gizmos draw antialiased `Line`s and filled convex `Quad`s (cones, plane squares,
cubes; shader mode 8); consecutive lines, quads and icons of one image batch into one instanced draw (both backends). After editing `shaders/ui.slang`, re-cook with
`shaders/compile.ps1` (slangc + spirv-val from VULKAN_SDK) and commit both .spv.

Play surfaces (D19 Game view, `shared.rs`): `import_play_surface(gpu, &SharedSurface)`
imports an embedded player's ring; `PlaySurface::show(ui, id, frame)` acquires the
slot, hands the previous one back (returned: release it via ara-play-host
`Surfaces`) and binds `id` through `external_image`; `close_play_surface` unbinds
before destroying. Re-import when the generation changes. Ignored GPU test in
`shared.rs`.

Floating editor windows (D23) share this adapter: `select_target(window)` swaps the
active render target (0 is the main window); textures, atlases and pipelines are
shared. Drop a closed window's target with `release_target` after queue idle.
Overlays apply only where the viewport image is drawn.

Glass (mode 11, D31): targets are also SAMPLED. A backdrop draw ends rendering, moves
the target to shader-read, draws `backdrop_band` into the same-size scratch (cleared),
then resumes the target with LOAD (its barrier grants attachment read) for the vertical
pass. Vulkan reserves two heap slots (target, scratch); Metal registers both images.
`tests/ui_pixels.rs` holds a satin/glass case against CpuGpu.
