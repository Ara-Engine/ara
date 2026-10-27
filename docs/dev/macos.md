# macOS (Apple silicon, Metal 4)

Ara on macOS uses its own Metal 4 backend (D12, D15): `ara_gpu::GpuDevice` is
`MetalDevice` there and `VulkanDevice` everywhere else. Only Apple silicon with the
Metal 4 GPU family is supported; the reference machine is a MacBook Pro M5 Pro.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| macOS | 26 or later | Metal 4. Verified on macOS 27 (26A5388g). |
| Xcode | 26 or later | Command-line tools and the macOS SDK; `xcode-select -p` must point at it. The Metal Toolchain component is **not** needed: the engine never compiles shaders at runtime. |
| Rust | pinned 1.99.1 | `rustup` installs it from `rust-toolchain.toml` on first use. |
| Vulkan SDK (LunarG) | 1.4.363.0 | Provides `slangc`, `spirv-val` and `dxc`. Install headless into your home directory, see below. Set `VULKAN_SDK=$HOME/VulkanSDK/1.4.363.0/macOS`. |
| .NET SDK | 10 | The C# host (`ara-script`, `dotnet/`). The macOS installer puts it in `/usr/local/share/dotnet`. |
| Metal Shader Converter | 4.0 or later, optional | Only on the canonical cook host (Windows, `C:Program FilesMetal Shader Converter`) to produce metallibs; on a Mac only for header/ABI verification. A signed-in Apple download: https://developer.apple.com/download/all/?q=metal%20shader%20converter. The macOS pkg installs to `/usr/local`; without admin rights, `pkgutil --expand-full` it into your home directory and set `ARA_METAL_SHADERCONVERTER` to the expanded `usr/local/bin/metal-shaderconverter`. |

Headless Vulkan SDK install (no admin rights, nothing outside the target directory):

```sh
mkdir -p ~/VulkanSDK/.download && cd ~/VulkanSDK/.download
curl -LO https://sdk.lunarg.com/sdk/download/1.4.363.0/mac/vulkansdk-macos-1.4.363.0.zip
shasum -a 256 vulkansdk-macos-1.4.363.0.zip   # compare with https://vulkan.lunarg.com/sdk/sha/1.4.363.0/mac/vulkansdk-macos-1.4.363.0.zip.txt
unzip -q vulkansdk-macos-1.4.363.0.zip
./vulkansdk-macOS-1.4.363.0.app/Contents/MacOS/vulkansdk-macOS-1.4.363.0 \
  --root "$HOME/VulkanSDK/1.4.363.0" --accept-licenses --default-answer \
  --confirm-command install copy_only=1
rm -rf ~/VulkanSDK/.download
```

Non-login shells (SSH, agents) do not read `/etc/paths.d`, so export the tools
explicitly:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/share/dotnet:$PATH"
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/macOS"
```

The vendored SDL3 builds its Cocoa, CoreAudio and IOKit/GameController backends
with the system compiler and links system frameworks; nothing else is installed.

## Commands

The same focused gates as on Windows (`AGENTS.md`), from the repository root:

```sh
cargo check -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --lib metal
```

Run Metal tests with API and shader validation on: validation failures abort the
process. On a laptop, check `pmset -g batt` before heavy builds and wrap long jobs
in `caffeinate -dimsu <command>` (macOS sleeps after a minute on battery).

## Shaders

- **Windows is the canonical cook host.** Never commit `cooked/` artifacts produced
  on a Mac. The Mac's `slangc` cannot load `dxc` for the mandatory DXIL target, and
  its build differs from the Windows one, so the cook key would differ even though
  the SPIR-V bytes are identical.
- Committed cooks are loaded unchanged: pipeline loading checks the schema, binding
  layout hash and per-artifact SHA-256, never the compiler key.
- Metal needs `CookedStage::metal` (metallib + converter reflection). The cook adds it
  only when Metal Shader Converter is installed (`ARA_METAL_SHADERCONVERTER`, `PATH`,
  or Apple's default location); otherwise it warns `ARA_GPU_METAL_COOK_SKIPPED`
  (ARA6007) and the Metal backend refuses those pipelines with `ARA_GPU_CONTRACT`.
- `crates/ara-shader-cook/metal/root-signature.json` is the one global root
  signature. It mirrors the Vulkan set-0 layout; the runtime side of the same
  layout is `crates/ara-gpu/src/metal/abi.rs`. The converter appends the three
  static samplers as a fifth table, so every stage's top-level argument buffer is
  32 constant bytes plus five table addresses (72 bytes).

## Status

| Area | macOS state |
|---|---|
| Kernel and platform-independent crates | Green (`cargo test`); the whole workspace type-checks. |
| SDL3 platform layer (`ara-platform`, `ara-audio`) | Green. |
| `ara-gpu` Metal 4 core | Buffers, textures, converter tables, pipelines from metallibs with a persistent pipeline archive, both queues with shared-event timelines (async compute), deferred completion, preparation workers, GPU pass timings and the timing window, CAMetalLayer presenter (windowless and through an SDL3 Metal view) that scales and converts engine output like the Vulkan blit, through MetalPerformanceShaders on a classic queue; shared surfaces publish scaled frames the same way. Tested with runtime-compiled MSL laid out like converter output and with the converter-cooked engine pipelines. Each device owns one 4096-slot timestamp heap (Metal allows 32 counter heaps per process); a submission without a free range runs untimed, and back-to-back pass boundaries share one timestamp. Queue barriers carry their accesses' stages, so a pass whose only dependencies are attachments runs its vertex (tiling) work under the previous pass's fragment work. Attachments whose contents are dead are neither loaded nor stored (graph discards, e.g. the G-buffer after ForwardTransparency; Outpost's pass went 0.688 → 0.667 ms), and `TRANSIENT_ATTACHMENT` images are memoryless. `set_pass_timing(false)` drops timestamps, but with 22 per frame left that measured no win, so players keep them. |
| Not yet on Metal | Acceleration structures/ray query, mesh stages, shader hot reload, HDR output, converter draw parameters (no cooked stage needs them yet). |
| Renderer, player, editor, Hub | The whole workspace builds and passes `clippy -D warnings` on macOS against `MetalDevice` (renderer seam: `GpuDevice` consumers, backend-selected `ara-platform::Present`, the Metal editor UI adapter in `ara-editor-gpu`). Every engine pipeline and the editor's `ui` pipeline carry Metal Shader Converter 4.0.3 metallibs, cooked on Windows. Under API and GPU validation, the whole `ara-render` suite passes on Metal (its captures within SSIM 0.005 of the Vulkan goldens), and so does the editor UI adapter's windowless GPU/CPU pixel test (`ara-editor-gpu` `tests/ui_pixels.rs`). The player, editor and Hub apps have not been run on Metal yet. Tests that drive raw Vulkan objects are Vulkan-only. |
| Player (`ara-player`) | Runs game packs exported on Windows on Metal, headless (`--headless`) and through the hidden continuous path (`--offscreen --frames N`), with C# gameplay as an osx-arm64 NativeAOT image and the native game UI. Showcase and Outpost captures are within SSIM 0.0011 and 0.0009 of the Windows captures at the same preset. On a Mac, `ara-export` exports `platform: macos` as an ad-hoc signed `.app` (crates/ara-export README, "macOS on a Mac"). |
| Crash capture (`ara-crash`) | Native crashes and Rust panics write `ara.crash/1` reports and minidumps to `~/Library/Application Support/Ara/crashes` (or `ARA_CRASH_DIR`), captured out of process like Windows and Linux. To symbolize, build with `CARGO_PROFILE_<PROFILE>_DEBUG=line-tables-only` and `CARGO_PROFILE_<PROFILE>_SPLIT_DEBUGINFO=packed`, run `ara-crash archive <binary>.dSYM/Contents/Resources/DWARF/<file> <store>`, then `ara-crash symbolize <dump> <store>`. |
| C# host (`ara-script`, osx-arm64) | Green: JIT hosting, NativeAOT sample, L1/L3 reload, the out-of-process compile server and module tests. Needs `dotnet` on `PATH`. |

## Running a Windows-built game

Until the macOS export lane exists, run an export or frozen pack made on Windows with
the Mac's player. Copy `game.arapack` and `ara-launch.json` into a folder. For a
game with C# gameplay, build its osx-arm64 image there (it writes `game/<Assembly>.dylib`
and `game/scripts.json`; the layout hash must match the Windows export's):

```sh
cargo run -p ara-export --bin ara-export -- scripts --project samples/outpost \
  --assembly Outpost --rid osx-arm64 --output PACK/game --job target/scripts-job
cargo run -p ara-player -- --root PACK --headless --quality high   # writes PACK/verify.png
```

- Compare against the Windows `verify.png` at the same `--quality`. `Auto` plans with
  Apple unified memory as device-local (Metal's recommended working set), so an
  M5 Pro picks High, by the same thresholds as a discrete GPU.
- Set `SDL_MAC_BACKGROUND_APP=1` for automated runs that open a hidden SDL window
  (`--offscreen`, editor and Hub verification routes): without it SDL makes the
  process a regular app, which shows a Dock icon and can take focus.

## Known issues

- Creating `MTL4Compiler`s while other compilers compile corrupted the heap inside
  the macOS 27 driver; the backend therefore shares one compiler per process, never
  destroyed.
- Pipeline archives: on macOS 27 a data-set serializer flush cleared what other live
  devices had captured, and compilers created or destroyed around other compiles
  crashed. So one compiler and serializer serve the process, and the archive is saved
  once, when the last device drops (shutdown for the player and editor). Archives
  captured under `MTL_SHADER_VALIDATION` fail to reload ("deployment target ... not
  compatible with current OS"), so validated processes neither load nor save them.
- Metal tests must run with Metal 4 validation on and be stress-checked in parallel
  (`cargo test` runs them concurrently), once more without `MTL_SHADER_VALIDATION`
  (only then do archives capture); use the exact binary from
  `cargo test --no-run --message-format=json` for Guard Malloc runs
  (`DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib`).
- Loopback UDP is delivered asynchronously on macOS: tests wait on delivery events,
  never on fixed sleeps.
- Vulkan backend GPU tests are ignored on macOS (no Vulkan driver; D12).
