# ara-platform

Minimal SDL3 native window/input and Vulkan presentation. Uses `sdl3-sys` 0.7.2
(SDL 3.4.18) with a raw SDL/Vulkan handle bridge compatible with git ash.

## Public API

- `Window`: main-thread-only resizable/high-DPI Vulkan window; owns an SDL video
  subsystem reference. `extensions()` returns owned strings, and `size()` returns
  physical pixels. It is intentionally neither Send nor Sync.
- `Event`: close, drawable resize, key/repeat, mouse motion/button and wheel input.
  Hosts translate these events into deterministic simulation actions.
- Multi-window: `Window::floating(title, rect, parent)` opens an owned tool window
  (above its parent, minimizes with it). `poll_tagged()` drains every window's
  events tagged with `id()` (0 = application; a close request is `Quit` for its
  window); `poll()` keeps the single-window shape. `rect`/`set_position`/`raise`/
  `toggle_fullscreen`/`display_scale`, `displays()` (usable bounds, for off-screen
  recovery) and `global_mouse()` (cross-window drags). `raw()` is for host adapters.
- Icon: `Window::new` applies the Ara app icon (`assets/brand/png/ara-icon-64.png`,
  embedded; failures are ignored because Wayland has no window icons). Games call
  `set_icon(rgba, [w, h])`. Regenerate the PNG with `cargo run -p ara-brand -- export`.
- `Present`: unsafe native integration over an existing GPU context. Retain both
  native owners, present only completed CopySource images, and destroy after idle.

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

`cargo test -p ara-platform --test native -- --ignored` opens a short-lived native
window/audio stream and checks injected key/mouse events, virtual gamepad
enumeration, recovery, and independent audio/video subsystem references.
Run `cargo test --workspace --no-run` after changing SDL linkage.

Behavioral gate from repository root:
`cargo run -p ara-render --example windowed -- --frames=120 --resize-test`.
Use `../ara-render/run.ps1 -Mode Window` for process-local loader isolation.
The example exercises the actual swapchain, dynamic pixel resize and renderer,
then exits with validation clean. Never leave the demo running after verification.
