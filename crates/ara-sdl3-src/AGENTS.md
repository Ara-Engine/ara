# ara-sdl3-src

One SDL 3.4.18 native library and one set of `sdl3-sys` bindings for platform
and audio. Consumers alias this package as `sdl3-sys`; do not depend directly
on the registry bindings or build a second SDL instance.

## Build and public API

- Reexports the SDL C bindings. `build.rs` builds committed C/C++ with parallel
  `cc`, without CMake, Python, pkg-config, downloads or installed desktop headers.
- `sdl3-sys/no-link` is intentional: this crate always emits the native link
  metadata, so workspace feature unification cannot suppress SDL linking.
- Default: static `ara_sdl3`. `SDL3_LIB_DIR` opts into a matching dynamic SDL3;
  its runtime library must then be available to the OS loader.
- Windows uses MSVC and SDK system libraries. Linux loads desktop/audio libraries
  dynamically; missing runtime libraries are diagnosed by SDL at initialization.
- Android uses the NDK compiler selected through normal `cc` environment variables.
  Packaging the SDL Java activity/USB integration is an Android application task.
- Apple source/config/framework selection is TODO(D15). Until then, use the
  explicit prebuilt override; the default reports the missing target support.

## Subsystems

Native video, Vulkan surfaces, events, keyboard/mouse/text/IME/clipboard/high-DPI,
joysticks/gamepads (HIDAPI, virtual devices), haptics, platform sensors, dialogs,
power, filesystem and SDL audio. No SDL renderer, GPU backend, camera or OpenGL.
Their central C API files remain because SDL's dynapi table references them.
SDL's renderer core also requires its YUV texture helpers even with every render
driver disabled; no SDL renderer, GPU or camera backend is enabled.
Linux IME uses XIM/Fcitx and Wayland text-input-v3; IBus/GLib is not compiled.

## Gates and maintenance

Set `CARGO_BUILD_JOBS=2`. Run `cargo check -p ara-sdl3-src`,
`cargo test -p ara-platform -p ara-audio`,
`cargo clippy -p ara-sdl3-src -p ara-platform -p ara-audio --all-targets -- -D warnings`,
and `cargo fmt -p ara-sdl3-src -p ara-platform -p ara-audio -- --check`.
Also link the workspace after changes to binding features.

See THIRD_PARTY_NOTICES.md for origins/licenses and manual protocol regeneration.
Keep vendor files plain UTF-8/LF text; no symlinks, archives, binaries or changelogs.

## Measurements (Windows x64, 2026-10-23)

Ryzen 7 7800X3D, MSVC 14.50, native optimization level 3 and debug symbols:
clean SDL C/C++ compilation + archives took 52.12 s with two Cargo jobs and
22.37 s with eight (Rust dependencies excluded; other agents were active).
WASAPI loopback, 32 trials: p50/p95 24.45/26.08 ms before, 25.31/26.35 ms after.
SDL WASAPI fallback, 8 trials: 92.48/93.72 ms before, 82.67/83.39 ms after.
Both use the existing QPC/MLS probe; output endpoint was BlackShark V3 - Game.

## Verified matrix (2026-10-23)

- Windows x64/MSVC: platform/audio/render/source tests, Clippy all targets with
  warnings denied, formatting, and `cargo test --workspace --no-run` passed after
  merging main at 3b9a091. Build jobs were two; only the timing comparison used eight.
- The explicit native test passed without any SDL DLL in the worktree: injected
  keyboard/mouse, virtual gamepad enumeration, audio recovery and owner lifetimes.
- The 120-frame windowed resize route exited validation-clean; its 960x540 capture
  was inspected. Audio/render executables have no SDL3.dll import or PATH entry.
- Linux x64/GNU: `cargo check -p ara-platform` and a statically linked SDL query
  executable passed. Auditing 271 native sources found no system desktop/audio
  development-header dependency. Native window/audio runtime was not exercised.
- Android ARM64/API 26, NDK 28.2: `cargo check -p ara-platform` passed; device
  packaging/runtime remains unverified. Apple source support remains TODO(D15).
- Vendor: 994 LF plain-text files, 18,574,364 bytes, above the 15 MB size aim;
  enabled subsystem sources, required API cores and declaration headers are retained.
