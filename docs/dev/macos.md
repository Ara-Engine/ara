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
| Metal Shader Converter | 3.x or later, optional | Only on the canonical cook host (Windows) to produce metallibs; on a Mac only for header/ABI verification. A signed-in Apple download: https://developer.apple.com/download/all/?q=metal%20shader%20converter. Installs to `/opt/metal-shaderconverter`. |

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
  layout is `crates/ara-gpu/src/metal/abi.rs`.

## Status

| Area | macOS state |
|---|---|
| Kernel and platform-independent crates | Green (`cargo test`); the whole workspace type-checks. |
| SDL3 platform layer (`ara-platform`, `ara-audio`) | Green. |
| `ara-gpu` Metal 4 core | Buffers, textures, converter tables, pipelines from metallibs, both queues with shared-event timelines (async compute), deferred completion, preparation workers, GPU pass timings and the timing window, CAMetalLayer presenter (windowless and through an SDL3 Metal view). Tested with runtime-compiled MSL laid out like converter output. |
| Not yet on Metal | Persistent pipeline archives, acceleration structures/ray query, mesh stages, `ClearDepth`, shader hot reload, HDR output, shared surfaces (D19 embedded Play), converter draw parameters. |
| Renderer, player, editor | Waiting on the renderer seam (`VulkanDevice` → `GpuDevice` across consumers) and on converter-cooked engine shaders. |
| C# host (`ara-script`, osx-arm64) | Green: JIT hosting, NativeAOT sample, L1/L3 reload, the out-of-process compile server and module tests. Needs `dotnet` on `PATH`. |

## Known issues

- Creating `MTL4Compiler`s while other compilers compile corrupted the heap inside
  the macOS 27 driver; the backend therefore shares one compiler per process, never
  destroyed.
- Pipeline archives (MTL4 data-set serializers) are not wired yet. On macOS 27 a
  serializer flush also cleared what other live devices had captured, and compilers
  created or destroyed around other compiles crashed. The intended design is one
  process-wide serializer flushed once, at shutdown, by single-device processes
  (player, editor).
- Metal tests must run with Metal 4 validation on and be stress-checked in parallel
  (`cargo test` runs them concurrently); use the exact binary from
  `cargo test --no-run --message-format=json` for Guard Malloc runs
  (`DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib`).
- Loopback UDP is delivered asynchronously on macOS: tests wait on delivery events,
  never on fixed sleeps.
- Vulkan backend GPU tests are ignored on macOS (no Vulkan driver; D12).
