# ara-platform

Minimal SDL3 native window/input and native presentation: Vulkan, or on macOS an SDL
Metal view presented through `ara_gpu::metal::present` (`src/present/metal.rs`). Uses `sdl3-sys` 0.7.2
(SDL 3.4.18) with a raw SDL/Vulkan handle bridge compatible with git ash.

## Public API

- `Window`: main-thread-only resizable/high-DPI Vulkan window; owns an SDL video
  subsystem reference. `extensions()` returns owned strings, and `size()` returns
  physical pixels. It is intentionally neither Send nor Sync.
- `Window::game`: borderless desktop fullscreen by default, explicit Windowed, or
  exclusive Fullscreen (closest display mode, desktop refresh). `set_game_mode`
  switches at runtime (game Settings menus) and `game_mode` reads it back; presenters
  follow the new drawable size. Hidden game windows never enter fullscreen. SDL
  fullscreen synchronization finishes before loading/swapchain sizing; direct scanout
  remains an OS/overlay decision.
- `Event`: close, drawable resize, key/repeat, mouse motion/button and wheel input.
  Hosts translate these events into deterministic simulation actions.
- `Window::set_cursor(SystemCursor)` lazily owns native cursor shapes until video
  shutdown. Resolve UI hover/capture every frame; offscreen hosts skip selection.
- Multi-window: `Window::floating(title, rect, parent)` opens an owned tool window
  (above its parent, minimizes with it). `poll_tagged()` drains every window's
  events tagged with `id()` (0 = application; a close request is `Quit` for its
  window); `poll()` keeps the single-window shape. `rect`/`set_position`/`raise`/
  `toggle_fullscreen`/`display_scale`, `displays()` (usable bounds, for off-screen
  recovery) and `global_mouse()` (cross-window drags). `raw()` is for host adapters.
- Custom captions: `Window::enable_custom_chrome` installs native caption/resize hit
  testing. Update `chrome_mut()` after layout in SDL window points. `minimize` and
  `toggle_maximize` retain native actions; other windows keep their existing frames.
- Icon: `Window::new` applies the Ara app icon (`assets/brand/png/ara-icon-64.png`,
  embedded; failures are ignored because Wayland has no window icons). Games call
  `set_icon(rgba, [w, h])`. Regenerate the PNG with `cargo run -p ara-brand -- export`.
- `Present`: backend-selected (`src/present/{vulkan,metal}.rs`). Portable surface:
  `create`, `create_with_output`, `present_image`, `present_encoded`, `retire` (no
  wait; callers wait for idle where they need it), `output_settings`, `surface_format`.
  Metal accepts SDR only and scales/converts like the Vulkan blit.
- Vulkan `Present`: unsafe native integration over an existing GPU context. Retain both
  native owners, present only completed CopySource images, and destroy after idle.
  Leaf acquire/present/queue-idle waits contribute to `Context::host_wait_stats`.
  Loading/game may transfer the same Present between owners of one native device;
  all uses must be complete first. `native_swapchain()` exposes its stable identity.

Per-image render semaphores are reused after reacquisition. Construction cleans
partially created surface/swapchain/semaphores; destruction is idempotent.
The v0 display path serializes presentation before releasing its command buffer.
The windowed example recreates on resize and OUT_OF_DATE; minimized windows wait.

## Setup and test

SDL 3.4.18 is built statically by `ara-sdl3-src` with parallel `cc`; no setup,
download, CMake, Python, `native/` directory or SDL3.dll is required. Audio uses
the same library. Consumers alias `ara-sdl3-src` as `sdl3-sys`; its explicit link
metadata survives workspace `no-link` feature unification.
`SDL3_LIB_DIR` is an opt-in dynamic prebuilt override (then provide its runtime).
Linux desktop/audio backends load host libraries dynamically; headers are vendored.
Android uses the NDK. Apple source builds remain TODO(D15), with a clear diagnostic
and the same prebuilt override available meanwhile.

Set `CARGO_BUILD_JOBS=2`. Run `cargo check -p ara-platform`,
`cargo test -p ara-platform`,
`cargo clippy -p ara-platform --all-targets -- -D warnings`, and
`cargo fmt -p ara-platform -- --check`.
The ignored `hidden_window_loads` lib test verifies all cursor resources without
showing a window or selecting a desktop cursor.

`cargo test -p ara-platform --test native -- --ignored` opens a short-lived native
window/audio stream and checks injected key/mouse events, virtual gamepad
enumeration, recovery, and independent audio/video subsystem references.
Run `cargo test --workspace --no-run` after changing SDL linkage.

Behavioral gate from repository root:
`cargo run -p ara-render --example windowed -- --frames=120 --resize-test`.
Use `../ara-render/run.ps1 -Mode Window` for process-local loader isolation.
The example exercises the actual swapchain, dynamic pixel resize and renderer,
then exits with validation clean. Never leave the demo running after verification.
`Window::hidden` / `floating_hidden` create windows that are never shown (offscreen
captures and verification); `raise` leaves hidden windows hidden.
