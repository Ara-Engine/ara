# ara-editor-gpu

Editor-only Vulkan UiGpu adapter over the renderer's existing ara-gpu device.
No project, scene-authority, widget or input state belongs here.

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
Do not enable runtime descriptor arrays: ara-gpu's device uses a fixed-size heap.
`ara-editor --verify` compares actual GPU output against CpuGpu for raster/MSDF,
nested clips, alpha, and complete editor screenshots with both semantic themes.

Set CARGO_BUILD_JOBS=2. Run cargo check/clippy/test -p ara-editor-gpu plus the
editor verification route, with process-local empty VK_IMPLICIT_LAYER_PATH.

Scene overlays are inserted immediately after the viewport image, before later
UI controls/popovers. Grid and hover execute on the GPU against the scene ID map.
The grid uses derivatives for antialiasing and distance fade, with theme axis tints.
Gizmos draw antialiased `Line`s and filled convex `Quad`s (cones, plane squares,
cubes; shader mode 8). After editing `shaders/ui.slang`, re-cook with
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
