# ara-platform

Minimal SDL3 native window/input and Vulkan presentation. Uses `sdl3-sys` 0.7.2
(SDL 3.4.18) with a raw SDL/Vulkan handle bridge compatible with git ash.

## Public API

- `Window`: main-thread-only resizable/high-DPI Vulkan window; owns an SDL video
  subsystem reference. `extensions()` returns owned strings, and `size()` returns
  physical pixels. It is intentionally neither Send nor Sync.
- `Event`: close, drawable resize, key/repeat, mouse motion/button and wheel input.
  Hosts translate these events into deterministic simulation actions.
- `Present`: unsafe native integration over an existing GPU context. Retain both
  native owners, present only completed CopySource images, and destroy after idle.

Per-image render semaphores are reused after reacquisition. Construction cleans
partially created surface/swapchain/semaphores; destruction is idempotent.
The v0 display path serializes presentation before releasing its command buffer.
The windowed example recreates on resize and OUT_OF_DATE; minimized windows wait.

## Setup and test

On Windows run `./setup.ps1` once, or provide `SDL3_LIB_DIR` at compile time.
`native/` is ignored. Put its SDL3.dll on PATH for native executables/tests.
On Linux/macOS install the matching system SDL3 development/runtime library.

Set `CARGO_BUILD_JOBS=4`. Run `cargo check -p ara-platform`,
`cargo test -p ara-platform`,
`cargo clippy -p ara-platform --all-targets -- -D warnings`, and
`cargo fmt -p ara-platform -- --check`.

Behavioral gate from repository root:
`cargo run -p ara-render --example windowed -- --frames=120 --resize-test`.
Use `../ara-render/run.ps1 -Mode Window` for process-local loader isolation.
The example exercises the actual swapchain, dynamic pixel resize and renderer,
then exits with validation clean. Never leave the demo running after verification.
