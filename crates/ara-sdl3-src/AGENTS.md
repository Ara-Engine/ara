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
Their central C API stub files remain because SDL's dynapi table references them.

## Gates and maintenance

Set `CARGO_BUILD_JOBS=2`. Run `cargo check -p ara-sdl3-src`,
`cargo test -p ara-platform -p ara-audio`,
`cargo clippy -p ara-sdl3-src -p ara-platform -p ara-audio --all-targets -- -D warnings`,
and `cargo fmt -p ara-sdl3-src -p ara-platform -p ara-audio -- --check`.
Also link the workspace after changes to binding features.

See THIRD_PARTY_NOTICES.md for origins/licenses and manual protocol regeneration.
Keep vendor files plain UTF-8/LF text; no symlinks, archives, binaries or changelogs.
