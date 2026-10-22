# R08 — Target platforms, GPU backend strategy, and the export pipeline

Research date: **2026-10-22/23**. Engine clones in `.refs/` were read and grepped only; nothing was built, run, or benchmarked. "Verified" means checked against a linked primary source, or against current source code (wgpu trunk, Godot 4.8-beta, Flax). Claims marked **(unverified)** come from secondary sources or memory. Recommendations are engineering judgments.

## TL;DR

- **GPU backends: build an engine-owned thin RHI with two native backends, Vulkan 1.3 (Windows, Linux, Android) and Metal 4 (macOS, iOS).** Add D3D12 only if evidence demands it. Do not build the game renderer on wgpu. NVRHI (from R02) is out: it is C++ and has no Metal backend.
- **Windows uses Vulkan, not D3D12, at launch.** One backend covers three OSes. Vulkan 2026 has the features we need: RT, mesh shaders, descriptor indexing, and the new D3D12-style `VK_EXT_descriptor_heap`. Known costs: no PIX, AMD's ML FSR features look DX12-first, and Xbox would need D3D12. Spike Electron viewport interop on Vulkan early.
- **Apple means Metal 4 only.** That requires Apple7+ (A14/M1) and macOS/iOS 26. Hardware RT and mesh shading start at Apple9 (M3/A17 Pro). macOS 26 is the last Intel release, so Intel Macs drop out entirely. Vulkan-on-Metal is not viable for high fidelity. MoltenVK is deprecated. KosmicKrisp runs on macOS only, has no iOS support, and Mesa lists no RT or mesh-shader support for it.
- **Shaders: Slang single-source.** Cook to SPIR-V for Vulkan and **DXIL → Apple Metal Shader Converter → metallib** for Apple. The converter runs on Windows and supports RT, mesh shaders, and SM6.6 bindless. Slang's own Metal target is experimental and lacks acceleration structures.
- **Rendering tiers are defined by capability, not platform:** **RT**, **Raster**, **Mobile**. All three share one render graph, one material AST, and one scene format. Baked/probe lighting is mandatory content because it is every tier's fallback.
- **Platform layer: SDL3 (via `sdl3-sys`)** for windows, input, gamepads, IME, touch, lifecycle, and audio devices. winit, gilrs, and cpal would leave gaps on mobile (gilrs has no Android support; winit 0.31 has been in beta since Nov 2025).
- **Export: two lanes.** The default lane is like Godot: a prebuilt, hash-pinned runtime template plus a cooked, content-addressed pack, with no compiling at export. An opt-in native lane compiles Rust modules per target. iOS packaging always ends on a Mac. macOS (outside the Mac App Store), Windows, Linux, and Android can be fully exported from Windows. macOS signing and notarization use `rcodesign`, as Godot already does.
- **iOS bans JIT. Every interpreter-based runtime works on all five platforms:** V8 jitless, QuickJS-ng, Hermes, Luau, Wasmi, and Wasmtime's Pulley interpreter. AOT-to-native options also work: Rust, wasm2c, and .NET NativeAOT (iOS still marked experimental by Microsoft). They force per-export compilation with a Mac for iOS. JIT-dependent paths do not work on iOS.
- **Agent-friendly export:** use `sn doctor` / `sn export` / `sn run` / `sn device …`. Output is an NDJSON event stream with stable error codes and machine-actionable fixes. Every dev build embeds an observability bridge (logs, screenshots, GPU timings, input injection). Agents can then verify builds identically on every platform. OS tools are used only to install, launch, and pull files.
- **Minimum farm:** GitHub-hosted Windows and Linux runners, plus a Linux container for Android and Steam Runtime builds. Self-hosted hardware: one Mac mini (Apple9/10) as CI runner, remote export agent, and iOS device host; the Windows RTX dev box as GPU runner; two or three Android phones on adb.

## Findings

### 1. GPU backend strategy

#### 1.1 What the platforms actually expose (Oct 2026)

| Platform | Native API | Relevant 2026 facts |
|---|---|---|
| Windows | D3D12, Vulkan | DXC shipped SM 6.9 in Feb 2026. SM 7 / SPIR-V-in-DirectX has still not shipped ([Phoronix](https://phoronix.com/news/dx-shader-compiler-better-vlk), [plan](https://phoronix.com/news/DirectX-Adopting-SPIR-V)). Vulkan 1.4.340 (Jan 2026) added `VK_EXT_descriptor_heap`, an explicitly D3D12-style heap model. NVIDIA R595 and Mesa RADV/ANV support it ([proposal](https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_descriptor_heap.html), [Phoronix](https://www.phoronix.com/news/Vulkan-1.4.340-Descriptor-Heap)). |
| Linux / Steam Deck | Vulkan | Mesa main lists RADV/ANV/NVK with Vulkan 1.4, `VK_KHR_ray_query`, and `VK_EXT_mesh_shader` (NVK has mesh but no RT) ([features.txt](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/features.txt)). |
| macOS | Metal 4 | Apple-silicon only from macOS 27. macOS 26 Tahoe is the last Intel release ([MacTech](https://www.mactech.com/2026/06/03/macos-tahoe-will-be-the-last-release-for-intel-based-macs/)). Apple's May 2026 feature table no longer has a Mac2 (Intel/AMD) column. |
| iOS/iPadOS | Metal 4 | Since 2026-04-28, App Store uploads must be built with Xcode 26 and the iOS 26 SDK ([Apple](https://developer.apple.com/news/upcoming-requirements/)). iOS 26 was on 79% of compatible iPhones by June 2026 per Apple's numbers ([9to5Mac](https://9to5mac.com/2026/06/10/ios-26-adoption-grows-but-still-lags-slightly-behind-ios-18/)). |
| Android | Vulkan (GLES legacy) | Google dashboard (data to Nov 2025): Vulkan 1.1 on 62%, 1.3 on 26%, 1.4 on 0.7%, no Vulkan on 7.4% of handhelds. ABP2022 covers 86.5% of Vulkan devices; the 2025 Android Vulkan Profile covers 80.1% ([dashboard](https://developer.android.com/about/dashboards)). Google's recommended engine floor is Android 10, Vulkan 1.1, and ABP2022 ([guide](https://developer.android.com/games/develop/vulkan/native-engine-support)). |

**Apple Silicon (Metal Feature Set Tables, dated 2026-05-21; [PDF](https://developer.apple.com/metal/Metal-Feature-Set-Tables.pdf))**

| Family | Chips | Metal 4 | Mesh shading | Ray tracing | BC textures | Notes |
|---|---|---|---|---|---|---|
| Apple6 | A13 | no | no | yes (compute/render pipelines, no RT hardware) | no | Below our floor |
| Apple7 | A14, M1 | **yes (floor)** | yes | yes (no RT hardware) | some (runtime check) | Metal 4 = argument tables, residency sets, command allocators, pipeline-dataset serialization, tensors |
| Apple8 | A15/A16, M2 | yes | yes | yes (no RT hardware) | some (runtime check) | Placement sparse resources |
| Apple9 | A17 Pro/A18, M3/M4 | yes | **hardware-accelerated** | **hardware-accelerated** ([tech talk](https://developer.apple.com/videos/play/tech-talks/111375/)) | all | 64-bit atomics, MetalFX denoised upscaling, intersection function buffers |
| Apple10 | A19, M5 | yes | yes | yes | all | Min/max sampler reduction, depth bounds, "universal texture compression" |

Metal 4 APIs need iOS/macOS 26 ([Apple docs](https://developer.apple.com/documentation/metal/understanding-the-metal-4-core-api)). Footnote 6 of the table says RT in render pipelines and function pointers do not work with mesh shading. Mac BC support on Apple7/8 is guaranteed only through `supportsBCTextureCompression`. In practice M1/M2 Macs expose it **(unverified across all SKUs)**.

#### 1.2 wgpu, checked against trunk code rather than docs

wgpu **30.0.1** (2026-08-21) is current. Its docs lag the code: `Features::EXPERIMENTAL_RAY_QUERY` still says "Vulkan" only, but the trunk `wgpu-hal` adapters enable ray queries on all three backends.

| Capability | Vulkan | DX12 | Metal | Evidence |
|---|---|---|---|---|
| Ray queries (inline RT) | yes | yes (Tier 1.1, SM6.5) | yes (`supports_raytracing`) | `wgpu-hal/src/{vulkan,dx12,metal}/adapter.rs`; Metal AS landed in v29, `intersection_query` in v30 |
| RT pipelines | yes | no (comment: "once supported…") | no | `dx12/adapter.rs` |
| Mesh shaders | yes | yes (naga HLSL done in v30) | yes (incl. WGSL, v30) | CHANGELOG v30 |
| Binding arrays ("bindless") | yes | textures/samplers | textures + buffer arrays (v30) | Features docs; Metal sampler arrays limited to 996/stage on Apple7/8 |
| Multi-draw-indirect-count | yes | yes | no | Features docs |
| Pipeline cache | yes | **no** | **no** | Features docs |
| Timestamps inside passes | yes | yes | **not on Apple GPUs** | Features docs |
| Multiple queues / async compute | **no** | **no** | **no** | [#1066](https://github.com/gfx-rs/wgpu/issues/1066) open since 2020; nothing merged |
| Passthrough shaders | SPIR-V | DXIL | MSL/metallib | `PASSTHROUGH_SHADERS` no longer experimental (v29) |

Sources: [Features](https://docs.rs/wgpu/latest/wgpu/struct.Features.html), [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).

wgpu's reach is real. Bevy 0.20 (2026-10-08) runs its Solari realtime path tracer on Metal, ships mesh shaders, and integrates DLSS via `dlss_wgpu`, which is Vulkan-only ([Bevy 0.20](https://bevy.org/news/bevy-0-20/), [dlss_wgpu](https://docs.rs/dlss_wgpu)).

Its ceiling is structural:
- one queue, so no async compute;
- WebGPU-style bind groups and automatic tracking;
- no user memory aliasing or sparse resources;
- no pipeline cache on Metal or DX12;
- RT is "experimental" with breaking changes. wgpu ships about four major versions a year.

#### 1.3 Vulkan-on-Metal is not a high-fidelity path

- **MoltenVK:** the README now carries a deprecation notice. Khronos recommends KosmicKrisp on Apple-silicon Macs and keeps MoltenVK only for Intel Macs and iOS ([MoltenVK](https://github.com/KhronosGroup/MoltenVK)). Its documented extension list has descriptor indexing but no ray query, acceleration structures, or mesh shaders ([user guide](https://github.com/KhronosGroup/MoltenVK/blob/main/Docs/MoltenVK_Runtime_UserGuide.md)).
- **KosmicKrisp (LunarG, in Mesa):**
  - Vulkan 1.4 conformant, presented at XDC 2026 and called "production ready on modern macOS" ([Phoronix](https://www.phoronix.com/news/KosmicKrisp-Vulkan-1.4-2026)).
  - Built on Metal 4, so it needs macOS 26+. No iOS support ([Mesa doc](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/drivers/kosmickrisp.rst)).
  - Mesa's feature list shows `kk` for core 1.0–1.3 and descriptor indexing, but **not** for `acceleration_structure`, `ray_query`, `mesh_shader`, `descriptor_buffer`, or `descriptor_heap` ([features.txt](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/features.txt)).
- **Verdict:** Vulkan-everywhere would make Apple9/10 Macs raster-only and leave iOS on a deprecated layer. It stays useful for development (running Vulkan validation on a Mac) and nothing more.

#### 1.4 Writing our own backends in Rust

| Crate | Status (crates.io, Oct 2026) | Note |
|---|---|---|
| `ash` | Last release **0.38.0+1.3.281 on 2024-04-01**; master is at `0.38.0+1.4.352` with commits to Jul 2026 | We would pin a git revision or vendor generated bindings. `vulkanalia` 0.35 (Feb 2026) is a maintained alternative |
| `objc2-metal` | 0.3.2 (2025-10) | Generated from Apple headers; check Metal 4 (`MTL4*`) coverage when spiking **(unverified)** |
| `windows` | 0.62.2 | D3D12 bindings, if ever needed |
| `blade-graphics` | 0.9.0 (2026-09) | Lean Rust RHI: Vulkan (including Windows), Metal, GLES. RT on Vulkan only. Good reference design ([blade](https://github.com/kvark/blade)) |
| `wgpu-hal` | MIT/Apache-2.0 | Proven code to borrow for swapchains, CAMetalLayer, Android surfaces, and driver workarounds |

#### 1.5 D3D12 vs Vulkan on Windows

| | Vulkan | D3D12 |
|---|---|---|
| Reuse | Same backend as Linux, Steam Deck, Android | Windows (and Xbox) only |
| High-end features | RT pipelines/queries, mesh shaders, NV cluster AS (exposed via wgpu as well), descriptor heap (new) | SM6.9, work graphs, DirectStorage, PIX |
| Vendor tech | DLSS on Vulkan verified (`dlss_wgpu`). FSR 4 / Redstone ML on Vulkan **not officially supported** per secondary reports ([gamegpu](https://en.gamegpu.com/news/igry/fsr-redstone-poluchil-podderzhku-v-sotnyakh-igr-na-videokartakh-radeon-rx-9000), [AMD forum](https://www.amdgaming.com/t/fsr4-and-fsr-redston-for-vulkan-games/93)). XeSS on Vulkan **(unverified)** | Everything ships here first |
| Tools | RenderDoc, Nsight, RGP | PIX, RenderDoc, Nsight, RGP |
| AAA precedent | id Software's RT-required PC titles (Doom: The Dark Ages requires hardware RT ([report](https://tech.yahoo.com/computing/articles/doom-dark-ages-might-send-225716354.html)); Vulkan-only status **unverified here**) | Most UE/Unity titles |
| Risks | Windows-on-Arm Vulkan driver quality **(unverified)**; Electron interop | One more backend to build |

The cost of adding D3D12 later is small *if* the shader pipeline already emits DXIL. Ours will, because Apple's converter consumes DXIL (§1.6).

#### 1.6 The shader path

- **Slang** (v2026.20, 2026-10-17):
  - Vulkan/SPIR-V and D3D12 targets are "supported". Metal is "experimental*" and WGSL "experimental**/WIP" ([README](https://github.com/shader-slang/slang)).
  - The Metal target doc maps `RaytracingAccelerationStructure` to **"(Not supported)"**. Mesh shaders and ParameterBlock → argument buffers do work ([Metal target](https://github.com/shader-slang/slang/blob/master/docs/user-guide/a2-02-metal-target-specific.md)).
  - Ray tracing and modules are stable. Mesh shaders and autodiff are experimental ([maturity](https://shader-slang.org/docs/feature_matureness/)).
- **Apple Metal Shader Converter** ([page](https://developer.apple.com/metal/shader-converter/)):
  - Converts DXIL → metallib, available as CLI and library. Current version is 4.0 beta, with **Windows and macOS hosts**.
  - Supports SM6.0–6.6: ray tracing (SM6.3), mesh/amplification (SM6.5), dynamic resources (SM6.6), and intersection function buffers.
  - Output needs Argument Buffers Tier 2 and macOS 14 / iOS 17 or later. Our Metal 4 floor already meets both.
  - Its runtime binding model is a top-level argument buffer that emulates root signatures and descriptor heaps, which is effectively D3D12 SM6.6 bindless.
- Apple also ships "Metal Developer Tools for Windows" (MSL → metallib), for compiling any hand-written MSL on Windows ([forum](https://developer.apple.com/forums/thread/671076)). Current version **unverified**.
- **Implication:**
  - One binding model everywhere: SM6.6-style heap indices plus root constants.
    - D3D12-native, if ever built.
    - On Vulkan, descriptor indexing, or `VK_EXT_descriptor_heap` where present.
    - On Metal, the converter's argument-buffer heap.
  - All shaders can be cooked on a Windows machine.
  - wgpu cannot consume converter output: its Metal backend has its own binding conventions. This is the decisive technical argument against "wgpu + Slang".

**Cook-time compilation.**
- Shader variants are compiled offline per (platform, tier, permutation): SPIR-V for Vulkan; DXIL then metallib for Apple; DXIL for D3D12 later.
- Each variant is stored in the content-addressed cache. PSO descriptions are collected from cooked materials (R02's "PSO manifests").
- At runtime:
  - Vulkan prewarms from the manifest and persists `VkPipelineCache`.
  - Apple ships precompiled pipeline data (binary archives / Metal 4 pipeline-dataset serialization, Apple7+). Offline GPU-binary generation per GPU family is **unverified for our toolchain** and needs a spike.
  - Android relies on prewarm plus driver caches.

### 2. Platform layer (Rust)

**Rust target tiers** ([platform support](https://doc.rust-lang.org/nightly/rustc/platform-support.html), checked 2026-10):

| Target | Tier | Floor |
|---|---|---|
| x86_64-pc-windows-msvc | 1 (host tools) | Windows 10 |
| aarch64-pc-windows-msvc | 1 (host tools) | — (cheap future option) |
| x86_64-unknown-linux-gnu | 1 (host tools) | glibc 2.17, kernel 3.2 |
| aarch64-apple-darwin | 1 (host tools) | macOS 11 |
| x86_64-apple-darwin | **2** (demoted) | macOS 10.12. Not needed: we are Apple-silicon only |
| aarch64-apple-ios / -ios-sim | 2 | Needs Xcode SDK |
| aarch64-linux-android / x86_64-linux-android | 2 | x86_64 only for emulators |

Tier 2 means "guaranteed to build", not "tests run in CI". Mobile Rust is still well-trodden (Firefox, Bevy, Tauri).

| Concern | Recommendation | Evidence / reasoning |
|---|---|---|
| Windowing, input, touch, pen, HiDPI | **SDL3** through `sdl3-sys` (0.7.2 tracks SDL 3.4.18, 2026-10), behind a small safe engine layer | SDL 3.4 shipped ~Jan 2026 ([Phoronix](https://www.phoronix.com/news/SDL-3.4-Released)). The higher-level `sdl3` crate (0.20) is still "mid-migration" ([docs.rs](https://docs.rs/crate/sdl3/latest)), so skip it. winit's last stable release is 0.30.13; 0.31 has been in beta since 2025-11. Bevy 0.19 still pins 0.30 |
| Gamepads | SDL3 gamepad DB, rumble, sensors, Steam Input | `gilrs` 0.11 marks **Android ✕** and lists no iOS ([gilrs](https://gitlab.com/gilrs-project/gilrs)) |
| IME / soft keyboard | SDL3 text-input API on all five. Game UI (R05) owns composition rendering | One API instead of per-backend gaps. Mobile IME depth (candidate windows, Android composition) needs a test plan **(unverified quality)** |
| Mobile lifecycle | Engine state machine driven by SDL3 lifecycle events. On background: stop GPU submission, drop the swapchain, flush saves | Android destroys surfaces on pause and may kill the process. iOS kills apps that submit GPU work in the background |
| File system | Engine VFS with mount points `pack://` (read-only, mmap), `user://` (saves), `cache://` (purgeable), `download://` | Android: APK assets plus Play Asset Delivery packs. iOS: read-only bundle, `Library/Caches`, Background Assets. Copies Godot's `res://` / `user://` idea |
| Audio devices | SDL3 audio streams, with the engine's own mixer on top | Avoids a second platform dependency. `cpal` 0.18 is a fine pure-Rust alternative (WASAPI, CoreAudio, ALSA/PipeWire/Pulse, AAudio on API 26+) ([cpal](https://github.com/RustAudio/cpal)) |
| Crash reporting | Desktop and Android: out-of-process minidumps with `crash-handler` 0.8 + `minidumper` 0.11 (Embark), uploaded to a Sentry-compatible endpoint (`sentry` 0.49). iOS: Sentry-Cocoa or an equivalent. Export archives PDB/dSYM/ELF debug symbols by build ID | iOS support in `crash-handler` **unverified** |
| Binary size | Track per platform in CI. JS engines dominate | rusty_v8 v152 static libs: ~20 MB gz (iOS), ~40 MB gz (desktop) ([releases](https://github.com/denoland/rusty_v8/releases)). App Store and Play limits are in §3.4 |

### 3. Export pipeline

#### 3.1 How the reference engines do it

| Engine | Model | Local / doc evidence |
|---|---|---|
| **Godot 4.8-beta** | Prebuilt, version-locked **export templates** plus a **PCK** pack. The pack can be embedded, encrypted, or sparse. Exports from CLI or editor; export presets are text | `editor/export/editor_export_platform.h:105-116` (encrypted/sparse PCK); `editor_export_platform_pc.cpp:67,314` (`binary_format/embed_pck`); `editor/export/shader_baker/` plus a per-platform `shader_baker/enabled` option (shader baking at export) |
| Godot: Apple | macOS signing choices are built-in ad-hoc, **`rcodesign`** (any host), or `codesign` (Mac only). Notarization runs through rcodesign or Apple's tools. The iOS export generates an Xcode project and runs `xcodebuild`. Deploy uses `xcrun devicectl device install app` / `process launch`, with an ios-deploy fallback | `platform/macos/export/export_plugin.cpp:83-180`; `editor/export/codesign.h:33-41` (ad-hoc only, no CMS); `editor/export/editor_export_platform_apple_embedded.cpp:2254-2297, 3060-3107` |
| Godot: Android | Prebuilt APK, re-signed with `apksigner`. **AAB/AAR requires "Use Gradle Build"**. Warnings are categorized messages | `platform/android/export/export_plugin.cpp:2053-2058, 2144, 2876-2884, 3139-3142` |
| **Flax** | Cooker steps: Validate → CompileScripts → CollectAssets → CookAssets → PrecompileAssemblies (.NET AOT) → DeployData → PostProcess. Platform tools per OS | `Source/Editor/Cooker/Steps/*`; `PrecompileAssembliesStep.cpp:14-36` (AOT modes, Mono AOT on iOS) |
| Flax: iOS/Android | iOS runs `/usr/bin/xcodebuild archive` + `-exportArchive` (Mac host). Android copies a Gradle project template, substitutes `${PackageName}` etc., then runs Gradle | `Platform/iOS/iOSPlatformTools.cpp:302-330`; `Platform/Android/AndroidPlatformTools.cpp:232-350` |
| **Unreal 5** | UAT BuildCookRun: compile (UBT), cook per platform, stage into IoStore `.utoc/.ucas` (Zen loader), package, archive. BuildGraph/Horde for farms. iOS from Windows via a "Remote Build" to a Mac over SSH/rsync | [Zen loader](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine) verified. Remote build only via [forum reports](https://forums.unrealengine.com/t/remote-build-from-windows-to-mac-doesnt-work/771179); BuildCookRun/BuildGraph details **unverified here** |
| **Unity 6** | Build Profiles. IL2CPP (C# → C++ → native, AOT; required for iOS). Generates Gradle and Xcode projects that users can edit. Optional cloud Build Automation | Well known; **not re-verified in this research** |

#### 3.2 Architecture: prebuilt runtime vs compile per export

The decisive variable is where gameplay code lives:
- **Bytecode or an interpreter-capable VM** means no compiler at export. The runtime binary is generic, and everything game-specific is data. That is fast exports, Windows-hosted exports for four of five platforms, and simple agent loops.
- **Native gameplay code** (Rust/C++/C#-AOT) means per-export compilation with each platform's toolchain, including Xcode on a Mac for every iOS build. Builds are slower, but performance is maximal.

**Recommendation: two lanes, one driver (`sn export`).**

```
resolve   project + export preset (text, in VCS) → platform profile (tier, formats, floors)
cook      incremental, content-addressed; same cache the editor uses
pack      manifest + chunk containers (deterministic, zstd, aligned), split into store asset packs
[native]  cargo build native modules for target (static on iOS); opt-in lane only
assemble  runtime template (hash-pinned to engine build) + pack + metadata (icons, plist/manifest)
sign      Authenticode | rcodesign + notary | apksigner/jarsigner | Mac agent: xcodebuild archive/export
verify    smoke launch (headless/emulator/device) → logs, screenshot, perf summary → report.json
```

- **Template lane (default).** Runtime templates are built by our CI for each (platform, arch, config: debug/profile/release). They are distributed content-addressed and fetched on demand per platform, not as one giant bundle.
  - iOS templates are a static library plus an Xcode project template.
  - Android templates are prebuilt modules. Exports re-link the manifest with `aapt2` and assemble with `bundletool`, without Gradle **(design to spike)**. Godot needs Gradle for AAB; we should not.
- **Native lane (opt-in).** For projects with native Rust modules or third-party native SDKs (ads, analytics, platform services), such as Godot GDExtension or Unity IL2CPP plugins. Plugin manifests declare per-platform native dependencies: Gradle dependencies, Swift packages, entitlements, Info.plist keys, permissions. The exporter merges them into the generated Gradle or Xcode project.
- Content is never compiled into binaries. A pack produced on Windows is valid for every platform whose cook profile it was built for.

#### 3.3 The iOS no-JIT constraint and which scripting runtimes survive

Apple does not grant third-party apps the JIT entitlement. Executable pages cannot be created at runtime, so loading AOT-compiled wasm (`.cwasm`) into memory is also out. Wasmtime's docs say Cranelift and Winch need on-the-fly executable memory and that Pulley is used where native compilation isn't available ([Wasmtime](https://docs.wasmtime.dev/stability-platform-support.html)).

Separately, Apple's developer agreement (§3.3.2, as quoted from the 2022 version) allows *downloaded interpreted code* only if it does not change the app's primary purpose, create a store, or bypass OS security ([forum quote](https://developer.apple.com/forums/thread/52161)). The 2026 text was **not verified**. Script hotfixes on iOS are possible but policy-sensitive.

| Runtime | iOS mode | Works on all 5? | Cost |
|---|---|---|---|
| **V8** (TypeScript/JS) | `--jitless` (Ignition interpreter) | Yes. rusty_v8 v152 ships `aarch64-apple-ios` and `-ios-sim` static libs, but **no Android prebuilt**, so Android means building V8 ourselves | Big: ~20–40 MB compressed static libs. Large iOS-vs-desktop speed gap. Best PC performance (JIT) |
| **QuickJS-ng** (`rquickjs` 0.14) | Interpreter everywhere | Yes | Small (~1 MB class). Much slower than JIT V8 **(no 2026 benchmark verified)**. Identical semantics on every platform |
| **Hermes V1** | Precompiled bytecode + interpreter | Yes in principle: proven on iOS/Android at React Native scale. V1 is the RN 0.84 default ([report](https://www.tothenew.com/blog/hermes-v1-by-default-in-react-native-0-84-the-biggest-performance-win-of-2026/)) | Desktop embedding from Rust is immature **(unverified)**. Static Hermes' typed AOT is not shipping ([SWM](https://swmansion.com/blog/welcoming-the-next-generation-of-hermes-67ab5679e184)) |
| **Luau** (`mlua` 0.12) | Interpreter; native codegen (x64/A64) is opt-in for embedders ([Luau](https://luau.org/2023/11/01/luau-recap-october-2023.html)) | Yes, Roblox-proven | Not TypeScript. TS → Luau transpilation (roblox-ts style) is possible but has semantic friction |
| **Wasm via Wasmi 2.0** (2026-09-01) | Pure interpreter | Yes | Any source language (Rust, C, AssemblyScript). Author claims ~2.2× faster than 1.0 and ahead of wasm3/WAMR/Pulley **(author's numbers)** ([coverage](https://zeli.app/story/49521031)) |
| **Wasm via Wasmtime 50** | Cranelift JIT on desktop/Android; **Pulley** interpreter on iOS | Yes ("iOS, Android supported but less well tested") | Two execution engines means two performance profiles |
| **wasm2c / AOT to C** | Native code linked at build time | Yes | Per-export native compile, including on a Mac for iOS (native lane) |
| **.NET NativeAOT** (C#) | AOT at build time | iOS is **experimental** in Microsoft's support table ([MS docs](https://learn.microsoft.com/dotnet/maui/deployment/nativeaot)). Android via NDK r27+ (Uno report) | Heavy toolchain plus per-export compile. Godot still marks C# on iOS/Android experimental and unsupported on web ([Godot docs](https://docs.godotengine.org/en/stable/tutorials/scripting/c_sharp/c_sharp_basics.html)) |
| **Rust gameplay** | Native | Yes | Per-export compile. No hot reload on iOS |
| JIT-only (V8 with JIT, LuaJIT JIT, CLR JIT, Mono JIT) | — | **No** | — |

**Platform verdict:**
- The runtime should be the *same engine in the same mode family* everywhere, with iOS only a flag. That rules out JavaScriptCore-on-Apple plus V8-elsewhere splits.
- Hot paths stay in Rust engine systems; scripts orchestrate.
- No JIT dependency is also the cheapest way to keep console doors open, since consoles generally restrict JIT too **(unverified per console)**.

The language choice itself belongs to the scripting research.

#### 3.4 Content per platform

| Content | PC (Win/Linux/macOS) | iOS | Android | Notes |
|---|---|---|---|---|
| Color/normal/mask textures | BC7 / BC5 / BC4 (+ zstd, RDO) | ASTC 4×4–8×8 per asset class | ASTC (ABP2022 requires `textureCompressionASTC_LDR` and ETC2) | Mac shares the PC cook if BC is present (runtime check, ASTC fallback). That is one desktop cook |
| HDR | BC6H | ASTC HDR **(Apple support unverified)** / RGB9E5 | ASTC HDR where supported, else RGB9E5/half | |
| Distribution compression | zstd per chunk | **XUASTC LDR** (Basis Universal v2.5, tag 2026-08-03: DCT-supercompressed ASTC in KTX2) | Same | ([Basis](https://github.com/BinomialLLC/basis_universal)). Ship target formats. Use KTX2/UASTC as the *universal* format for UGC/web later |
| Audio | Opus for music/dialog; PCM/ADPCM for short latency-critical SFX | Same | Same | One codec set. Mobile gets a quality preset, not different codecs |
| Shaders | SPIR-V | metallib (+ pipeline data) | SPIR-V (mobile-tier variants) | Per (platform, tier) |
| Packs | Content-addressed chunks + manifest (IoStore-like) | Install-time pack + **Background Assets** (ODR deprecated as of iOS 27 ([ASC](https://developer.apple.com/help/app-store-connect/reference/on-demand-resources-size-limits/))) | Install-time / fast-follow / on-demand asset packs | Play limits: base module 200 MB; asset pack 1.5 GB; 4 GB cumulative per class ([Play](https://support.google.com/googleplay/android-developer/answer/9859372)). Background Assets limits are reported as 200 GB / 100 packs **(forum, unverified)** |

**Patching:**
- Manifests are diffed by chunk hash.
- Pack output must be **byte-deterministic with stable ordering**, so Steam/Play/App Store deltas stay small.
- An optional engine CDN downloader handles live *data*. On iOS, script updates must stay within the §3.3.2 limits.
- Pack encryption is deterrence only: the key lives in the binary.

### 4. Building and signing from a Windows-first workflow

| Platform | Toolchain | Signing | Fully from Windows? | Store gates (2026) |
|---|---|---|---|---|
| Windows | MSVC target, static CRT | Authenticode. **Azure Artifact Signing** (~$9.99/mo; individuals US/Canada only; SmartScreen reputation still builds over time; EV no longer bypasses it) ([MS](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)) | Yes | Steam/itch: signing optional. Steam downloads generally don't trigger SmartScreen **(unverified)** |
| Linux | Build in the **Steam Runtime 3 "sniper" SDK container** (Debian 11, glibc 2.31) via CI or WSL2. SDL3 handles Wayland and X11 | none | Yes (container) | Steam Runtime 4.0 (Debian 13) exists and drops GTK/Qt; SDL3 is recommended ([GoL](https://www.gamingonlinux.com/2025/11/valve-put-up-a-new-steam-linux-runtime-4-0-with-a-move-towards-64-bit)) |
| macOS | Prebuilt arm64 `.app` template (built by our Mac CI) | Developer ID + notarization via **`rcodesign`** on Windows (sign, notarize, staple for bundles, dmg, pkg) ([rcodesign](https://gregoryszorc.com/docs/apple-codesign/main/)). Godot integrates this | **Yes** for Steam/itch/direct (zip). Mac App Store `.pkg` and dmg creation → Mac | Notarization required for direct distribution |
| iOS | Xcode 26 / iOS 26 SDK required | Distribution cert + profiles; `xcodebuild -allowProvisioningUpdates` with an App Store Connect API key | **No.** Xcode and SDKs are licensed for Apple hardware **(license text not re-read)** and needed for SDK stamps, asset catalogs, and upload | TestFlight / App Store |
| Android | NDK (`cargo-ndk` 4.1); arm64 release, x86_64 dev-only | Upload keystore + Play App Signing; `apksigner` for side-loaded APKs | Yes | AAB; **target API 36 since 2026-08-31** ([Play](https://developer.android.com/google/play/requirements/target-sdk)); 16 KB page-aligned `.so` (dates conflict across sources: Nov 2025 vs May 2026 ([fCC](https://www.freecodecamp.org/news/google-16-kb-page-size-requirement-what-to-do/))). Link with `-z max-page-size=16384` |

**Ways to get a Mac:**
1. **Self-hosted Mac mini.** Recommended: it's also the Metal GPU test box and the iOS device host.
2. **GitHub-hosted macOS runners**: $0.062/min standard, $0.102/min M2 Pro XL ([GitHub](https://docs.github.com/en/enterprise-server@3.17/billing/reference/actions-runner-pricing)). No guaranteed real-GPU testing **(unverified)**.
3. **AWS EC2 Mac**: 24-hour minimum host allocation, required by Apple's macOS license ([AWS FAQ](https://aws.amazon.com/ec2/instance-types/mac/faqs/)).

Cross-compiling iOS on Windows or Linux with copied SDKs is technically possible but breaks Apple's license and SDK stamping. Reject it.

### 5. Agent-friendly export and device testing

**CLI surface.** One binary (`sn`) is shared by the editor's native host, CI, and agents. Every command takes `--json`.

- `sn doctor [--platform P]` checks SDK/NDK/JDK versions, keystores, certificates, the Mac agent's reachability, Xcode version, and devices.
- `sn export --platform {windows|linux|macos|android|ios} --profile release [--out D]`
- `sn run --platform P --device ID [--scenario smoke.yaml] [--capture screenshot,perf:10s]`
- `sn device list|logs|screenshot|pull|perf`
- `sn crash symbolicate dump.dmp`

**Error contract** (NDJSON events: `progress`, `artifact`, `warning`, `error`):

```json
{"type":"error","code":"SN-EXP-IOS-012","stage":"sign",
 "message":"No iOS distribution certificate for team ABCD1234 on agent mac-mini.",
 "fixes":[{"kind":"command","run":"sn apple certs import --p12 <path> --agent mac-mini"},
          {"kind":"doc","ref":"sn://docs/export/ios/signing"}],
 "context":{"agent":"mac-mini","team_id":"ABCD1234","profile":"release"}}
```

Codes are stable and documented. Fixes are executable. Context is typed. Godot's categorized `add_message(...)` calls are the human-oriented precursor (`export_plugin.cpp:3142`).

**Device verification:**
- **Build observability into the runtime.** Dev and profile builds embed an **sn-bridge** server (WebSocket over TCP, through `adb forward` on Android and LAN or usbmux on iOS). It exposes:
  - structured logs;
  - swapchain screenshots;
  - frame and GPU timestamp histograms;
  - scene and UI semantic queries (R05);
  - input injection;
  - capture triggers.
- OS tooling is used only where unavoidable:

| Task | Android | iOS device | iOS Simulator | Desktop |
|---|---|---|---|---|
| Install / launch | `adb install`, `am start` | `xcrun devicectl device install app` / `process launch` (as Godot does) | `simctl install/launch` | spawn |
| Logs | bridge + `logcat` (crashes) | bridge + pull files via `devicectl` | bridge + `simctl spawn log` | bridge + stdout |
| Screenshot | bridge (or `adb exec-out screencap`) | **bridge**: devicectl has no screenshot subcommand; pymobiledevice3 DVT needs a root tunnel ([community](https://tessl.io/registry/skills/github/NeverSight/learn-skills.dev/ios-device-screenshot)) | bridge / `simctl io screenshot` | bridge |
| Perf capture | Perfetto, AGI | Instruments `xctrace`; WWDC26 adds a `metalperftrace` CLI with JSON output (macOS 27) ([summary](https://daily.dev/posts/wwdc26-find-and-fix-performance-issues-in-your-metal-games-apple-focakni4v), secondary) | limited | PIX n/a; RenderDoc/Nsight; Xcode GPU capture |

**Simulator and emulator limits:**
- The iOS Simulator advertises only the **Apple2** GPU family (wgpu changelog). Metal 4 there was flagged as a regression on the forums ([thread](https://developer.apple.com/forums/thread/802172)). Our Apple7/Metal 4 renderer may not run there at all, so use it for logic/UI tests with a null renderer and test GPU work on devices.
- The Android Emulator forwards Vulkan via gfxstream. Since 36.4.9 (Feb 2026) its software renderer defaults to Lavapipe ([notes](https://developer.android.com/studio/releases/emulator)). It's fine for smoke tests, not GPU sign-off. Real-device farms (Firebase Test Lab, AWS Device Farm) are for pre-release breadth.

### 6. Editor platforms

R04 picks an Electron shell around a native host, which is cross-OS by construction. Platform-specific parts:
- **viewport interop:** Windows shared handles, macOS IOSurface, Linux dmabuf;
- **cook tools per host:** Slang and DXC everywhere; Metal Shader Converter on **Windows and macOS only**.

| Editor host | When | Can export to |
|---|---|---|
| Windows | v0 (owner's machine, primary GPU dev) | Windows, Linux (container), Android, macOS (rcodesign), iOS via Mac agent |
| macOS (Apple silicon) | v1. Cheap with Electron; needed for Apple-native debugging (Xcode GPU capture) | All |
| Linux | v1.x, "best effort" CI artifact | Windows, Linux, Android; Apple via Mac agent (the converter has no Linux host) |

On Windows, Electron imports **D3D shared textures**. With a Vulkan backend this means exporting Vulkan memory as a D3D11-compatible NT handle (`VK_KHR_external_memory_win32`) **(unspiked)**. That is the one place D3D12 on Windows might win, so spike it in month 1.

### 7. Godot as the model

**Copy:**
- single-binary editor plus downloadable, version-locked templates;
- the pack separate from the runtime, with optional embedding;
- text export presets in VCS;
- headless CLI export;
- one-click deploy over adb/devicectl;
- `rcodesign` for Apple signing on any host;
- Android export without Gradle for iteration;
- a shader baker at export;
- categorized export messages.

**Fix:**
1. Templates are monolithic, must match the editor version exactly, and are huge. Ours are content-addressed and fetched per platform on demand.
2. Size stripping, custom modules, and encryption keys need custom templates compiled from source. Our native lane does that with the normal `sn export`, and pack keys are not baked in.
3. iOS export requires the user to own a Mac session. Ours is a remote Mac agent inside `sn export`.
4. C# is second-class on iOS/Android/web. Pick a scripting runtime that is first-class on all targets from day one.
5. AAB needs Gradle. Ours uses `aapt2 + bundletool`, with Gradle only for plugins.
6. Native SDKs come in through ad-hoc plugins. Ours use typed plugin manifests merged into the Gradle and Xcode projects.
7. Errors are human strings. Ours are coded and actionable (§5).

## Recommendations for snengine

### Platform × feature-tier matrix

Tiers are capability sets checked at startup. Content is authored once.

| | **RT** (PC/Mac-High) | **Raster** (Standard) | **Mobile** |
|---|---|---|---|
| Required caps | Bindless heap, HW ray query, timeline sync, compute/indirect; mesh shaders optional | Bindless heap, compute/indirect, timestamps | Bindless (descriptor indexing), ASTC, tile-friendly passes |
| Lighting | Baked/probe base + RT-DDGI, RT reflections/shadows; later stochastic many-lights | Baked + probes + SSGI/SSR, shadow cascades | Baked lightmaps/probes, simplified shadows |
| Geometry | Meshlets (mesh shaders or compute raster), LOD | Meshlets via indirect + LOD | LOD, instancing |
| Upscaling | DLSS / FSR / XeSS / MetalFX denoised (Apple9) | FSR / MetalFX temporal / own TAA-U | MetalFX spatial/temporal, own TAA-U |
| Windows x64 (Vulkan) | RTX 20+, RX 6000+, Arc | older dGPU / iGPU | — |
| Linux x64 + Steam Deck (Vulkan, sniper) | dGPU with Mesa/NVIDIA RT | **Deck default** | — |
| macOS 26+ arm64 (Metal 4) | **M3/M4/M5 (Apple9/10)**, budgeted | M1/M2 (Apple7/8) | — |
| iOS 26+ (Metal 4) | later: A17 Pro+/A18/A19 | M-series iPads (optional) | **A14+ (Apple7)** |
| Android 10+ arm64 (Vulkan) | later: RT-capable flagships | — | **Vulkan 1.1+ with descriptor indexing + ASTC** (coverage to measure) |
| Texture cook | BCn | BCn | ASTC (XUASTC for download size) |
| Shader cook | SPIR-V / metallib | SPIR-V / metallib | SPIR-V / metallib |

**The rule that makes one renderer possible:**
- Every scene must have a valid baked/probe lighting solution, because Raster and Mobile depend on it.
- Each render-graph feature node (GI, reflections, shadows, upscale) has per-tier implementations behind one interface.
- Materials lower per tier from the shared AST, with explicit cook-time diagnostics for anything unsupported.

### GPU backend strategy: the call

**Own thin RHI ("sn-gpu"), with native Vulkan 1.3 and Metal 4 backends. Not wgpu. D3D12 only on evidence.**

1. **Binding model:** SM6.6-style global resource and sampler heaps plus root constants. On Vulkan: descriptor indexing (one update-after-bind set), using `VK_EXT_descriptor_heap` where available. On Metal: the shader converter's argument-buffer heap.
2. **Sync model:** the render graph emits explicit barriers. Graphics, async-compute, and copy queues are synchronized with timeline semaphores / `MTLSharedEvent`. Transient-resource aliasing is owned by the graph.
3. **Pipelines:** created from cooked descriptions and persistently cached (VkPipelineCache; Metal binary archives / pipeline datasets).
4. **Escape hatches:** raw handles for DLSS/Streamline, FSR, XeSS, and MetalFX.
5. **Order:** Vulkan backend first (Windows dev box, then Linux CI, then Android); Metal 4 second (months 3–6).
6. **Validation:** an agent-written "RHI conformance suite" runs on real GPUs with Vulkan validation (sync and GPU-assisted) and Metal API/shader validation enabled. Agents produce bugs; validation turns them into errors.
7. **Trigger to add D3D12:** Electron interop fails on Vulkan, Windows-on-Arm matters, DX12-only vendor features (e.g. AMD ML frame generation) become product-critical, or Xbox comes into scope. The shader pipeline already emits DXIL, so only the backend is new work.

**Why not wgpu:**
- It works today and is the right choice if web becomes a target or if all five platforms must render in month one.
- But its single queue, bind-group plus auto-tracking model, missing Metal/DX12 PSO caches, and experimental RT cap a UE-class renderer.
- Slang/converter output doesn't fit its Metal binding conventions, which would force WGSL as the long-term shader language.
- Both ceilings are hard to reverse once hundreds of passes and shaders are written against them.

**If the owner prefers wgpu anyway:** commit fully. Use WGSL with wgpu extensions, compile through naga, and accept the cap.

### Export architecture

- Two lanes as in §3.2. The template lane is the default.
- Runtime templates are content-addressed CI artifacts per (platform, arch, config). Packs are deterministic chunked containers with platform cook keys `(asset GUID, platform profile, cooker version)`.
- Windows exports Windows, Linux, Android, and macOS by itself. iOS (and Mac App Store) runs through the Mac agent, using the same `sn` binary over SSH.
- Symbols are archived by build ID. Every export ends with a `verify` stage that produces `report.json`.

### Minimal build farm / CI for a Windows-first team

1. **GitHub-hosted runners:**
   - Windows: engine and runtime templates, unit tests.
   - Ubuntu: Linux templates built inside the sniper SDK container; Android templates via NDK.
   - Hosted macOS: overflow only.
2. **Self-hosted Mac mini (M4/M5, Apple9/10):**
   - macOS and iOS templates;
   - Metal conformance and GPU tests;
   - iOS packaging and TestFlight upload;
   - `sn agent` for users' iOS exports;
   - hosts one current iPhone plus one A14/A15 floor device.
   - Keychain holds the Apple certificates. CI uses an App Store Connect API key.
3. **Self-hosted Windows GPU runner** (the RTX dev box at night, then a dedicated box; add AMD and Intel GPUs over time). It also hosts 2–3 Android phones over adb: an Adreno flagship, a Mali mid-range, and a low-end floor device.
4. **Secrets:**
   - Azure Artifact Signing (or an OV certificate in a cloud HSM);
   - Android upload keystore;
   - Apple Developer ID `.p12` plus notary API key, used by `rcodesign` on Windows/Linux CI.
5. **Pre-release:** a cloud device-farm pass (Firebase Test Lab or AWS Device Farm) for breadth.

### Scripting runtimes viable on all five platforms

- **Viable, interpreter modes:** V8 (jitless on iOS; Android build is ours), QuickJS-ng, Hermes V1, Luau, Wasmi, Wasmtime + Pulley.
- **Viable via per-export AOT (native lane, Mac needed for iOS):** Rust, wasm2c, .NET NativeAOT (iOS experimental).
- **Not viable:** anything that needs JIT on iOS.
- **Platform recommendation:** one engine and semantics everywhere, interpreter-capable, with iOS as a mode flag. Treat interpreter-only iOS performance as a design budget. Measure V8-jitless vs QuickJS-ng vs Luau on A14 in the scripting spike.

### The 5 platform decisions hardest to change later

1. **The RHI's binding and synchronization model** (bindless heaps, explicit graph barriers, multi-queue) and owning backends vs using wgpu. It leaks into every pass and shader.
2. **Shader language and cook pipeline** (Slang single-source, DXIL as the Apple bridge, reflection-generated layouts). Every engine shader and every material lowering depends on it.
3. **Gameplay runtime and its iOS execution mode.** This decides template vs per-export compile, hot reload, binding generation, and the agent loop.
4. **Pack format and asset identity** (content-addressed chunks, stable GUIDs, deterministic output, platform cook keys). Patching, DLC, store delivery, and save compatibility all depend on it.
5. **Platform floors and lifecycle ownership:**
   - floors: Metal 4/Apple7 + OS 26; Android Vulkan profile with bindless; arm64-only mobile; Apple-silicon-only Mac;
   - lifecycle: SDL3-driven main loop, iOS main-thread rules, Android surface loss.

   Retrofitting mobile lifecycle or lowering a floor after the renderer depends on it is notoriously painful.

## Open questions / risks

- **This report conflicts with R07 (`07-rust-path.md`).** R07 recommends a wgpu-first vertical slice, adding one raw `ash` backend only when a blocker is measured. I recommend owning the Vulkan and Metal 4 backends from the start. The disagreement turns on two Apple-specific facts R07 did not weigh: (a) Slang's Metal target lacks acceleration structures, while the DXIL → Metal Shader Converter route that has them does not fit wgpu's Metal bindings; (b) the binding and sync model is hard to reverse. Resolve with a two-week spike that ports one RT-GI pass plus one bindless material to wgpu-Metal and to converter-fed Metal 4. Whichever compiles from the *same Slang source* with less glue wins. R07's V8/`deno_core` scripting choice also needs an Android V8 build: rusty_v8 ships no Android prebuilt.
- **Descriptor-indexing coverage on Android** is not guaranteed by ABP2022 or VPA15/16 (neither profile JSON requires it). Measure it with the Play Console device catalog before fixing the Mobile floor. A non-bindless mobile path would undermine "one renderer".
- **Metal Shader Converter:** a closed Apple binary. Redistribution terms for bundling it in the snengine editor were **not verified**. If they forbid it, users download it once, which `sn doctor` can automate. Its binding model must be spiked against objc2-metal plus Metal 4 argument tables.
- **Slang → SPIR-V bindless lowering** (SM6.6 `ResourceDescriptorHeap`-style) needs a spike on Vulkan, including Android drivers.
- **Electron viewport interop with a Vulkan backend on Windows** is unproven (R04's top risk compounds here).
- **AMD ML upscaling/frame generation on Vulkan:** possibly DX12-only for a while. Mitigate with FSR 3.x and our own TAA-U.
- **ash has had no crates.io release since 2024.** Pin a git revision, or use vulkanalia.
- **iOS Simulator** may not run a Metal-4-floor renderer. Plan device-first iOS testing.
- **Play 16 KB alignment deadline dates** conflict across sources. The Background Assets size limits are forum-reported.
- Not verified here: Steam's SmartScreen behavior, XeSS Vulkan support, Doom: The Dark Ages being Vulkan-only, and Unity/Unreal pipeline specifics beyond the Zen loader.
- Live script patching on iOS relies on DPLA wording verified only in its 2022 form.

## Sources

**Local evidence (read-only clones, HEADs of 2026-10-22):**
- **G1** `.refs/godot/version.py` (4.8-beta)
- **G2** `.refs/godot/editor/export/editor_export_platform.h:105-116`
- **G3** `.refs/godot/editor/export/editor_export_platform_pc.cpp:67,314-324`
- **G4** `.refs/godot/platform/macos/export/export_plugin.cpp:83-220`
- **G5** `.refs/godot/editor/export/codesign.h:33-41`
- **G6** `.refs/godot/platform/android/export/export_plugin.cpp:2048-2060,2144,2876-2884,3139-3142`
- **G7** `.refs/godot/editor/export/editor_export_platform_apple_embedded.cpp:2254-2297,3023-3107`
- **G8** `.refs/godot/editor/export/shader_baker/`
- **F1** `.refs/flax/Source/Editor/Cooker/Steps/`
- **F2** `.refs/flax/Source/Editor/Cooker/Platform/iOS/iOSPlatformTools.cpp:302-330`
- **F3** `.refs/flax/Source/Editor/Cooker/Platform/Android/AndroidPlatformTools.cpp:232-350`
- **F4** `.refs/flax/Source/Editor/Cooker/Steps/PrecompileAssembliesStep.cpp:14-36`

**GPU/API:**
- wgpu: [Features](https://docs.rs/wgpu/latest/wgpu/struct.Features.html), [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md), [wgpu-hal backends](https://github.com/gfx-rs/wgpu/tree/trunk/wgpu-hal/src), [multi-queue #1066](https://github.com/gfx-rs/wgpu/issues/1066)
- Apple: [Metal Feature Set Tables (2026-05-21)](https://developer.apple.com/metal/Metal-Feature-Set-Tables.pdf), [Metal 4 core API](https://developer.apple.com/documentation/metal/understanding-the-metal-4-core-api), [Apple9 tech talk](https://developer.apple.com/videos/play/tech-talks/111375/), [Metal shader converter](https://developer.apple.com/metal/shader-converter/), [Metal tools for Windows](https://developer.apple.com/forums/thread/671076), [Metal 4 on Simulator](https://developer.apple.com/forums/thread/802172)
- Vulkan-on-Metal: [MoltenVK (deprecation notice)](https://github.com/KhronosGroup/MoltenVK), [MoltenVK user guide](https://github.com/KhronosGroup/MoltenVK/blob/main/Docs/MoltenVK_Runtime_UserGuide.md), [KosmicKrisp 1.4](https://www.phoronix.com/news/KosmicKrisp-Vulkan-1.4-2026), [KosmicKrisp Mesa doc](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/drivers/kosmickrisp.rst), [Mesa features.txt](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/features.txt)
- Vulkan and DirectX: [VK_EXT_descriptor_heap](https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_descriptor_heap.html), [Vulkan 1.4.340](https://www.phoronix.com/news/Vulkan-1.4.340-Descriptor-Heap), [DXC SM6.9](https://phoronix.com/news/dx-shader-compiler-better-vlk), [DirectX SPIR-V plan](https://phoronix.com/news/DirectX-Adopting-SPIR-V)
- Slang: [README](https://github.com/shader-slang/slang), [Metal target](https://github.com/shader-slang/slang/blob/master/docs/user-guide/a2-02-metal-target-specific.md), [maturity](https://shader-slang.org/docs/feature_matureness/)
- Upscalers and precedent: [Redstone/Vulkan (secondary)](https://en.gamegpu.com/news/igry/fsr-redstone-poluchil-podderzhku-v-sotnyakh-igr-na-videokartakh-radeon-rx-9000), [AMD forum](https://www.amdgaming.com/t/fsr4-and-fsr-redston-for-vulkan-games/93), [Doom TDA RT requirement](https://tech.yahoo.com/computing/articles/doom-dark-ages-might-send-225716354.html), [dlss_wgpu](https://docs.rs/dlss_wgpu), [Bevy 0.20](https://bevy.org/news/bevy-0-20/), [blade](https://github.com/kvark/blade)

**Platforms and stores:**
- Android: [dashboard](https://developer.android.com/about/dashboards), [native engine guide](https://developer.android.com/games/develop/vulkan/native-engine-support), [Vulkan profiles](https://github.com/KhronosGroup/Vulkan-Profiles/tree/main/profiles), [target SDK](https://developer.android.com/google/play/requirements/target-sdk), [16 KB (secondary)](https://www.freecodecamp.org/news/google-16-kb-page-size-requirement-what-to-do/), [Play size limits](https://support.google.com/googleplay/android-developer/answer/9859372), [emulator notes](https://developer.android.com/studio/releases/emulator)
- Apple: [SDK requirement](https://developer.apple.com/news/upcoming-requirements/), [ODR deprecation](https://developer.apple.com/help/app-store-connect/reference/on-demand-resources-size-limits/), [Background Assets forum](https://developer.apple.com/forums/thread/810659), [Intel end](https://www.mactech.com/2026/06/03/macos-tahoe-will-be-the-last-release-for-intel-based-macs/), [Rosetta timeline](https://www.techrepublic.com/article/news-apple-macos-27-drops-intel-mac-support-rosetta/), [iOS 26 adoption](https://9to5mac.com/2026/06/10/ios-26-adoption-grows-but-still-lags-slightly-behind-ios-18/), [DPLA 3.3.2 quote](https://developer.apple.com/forums/thread/52161)
- Linux, Windows, CI: [Steam Runtime 4](https://www.gamingonlinux.com/2025/11/valve-put-up-a-new-steam-linux-runtime-4-0-with-a-move-towards-64-bit), [Windows code signing](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options), [rcodesign](https://gregoryszorc.com/docs/apple-codesign/main/), [AWS EC2 Mac](https://aws.amazon.com/ec2/instance-types/mac/faqs/), [GitHub runner pricing](https://docs.github.com/en/enterprise-server@3.17/billing/reference/actions-runner-pricing)
- Rust and engines: [Rust platform support](https://doc.rust-lang.org/nightly/rustc/platform-support.html), [Godot C# platforms](https://docs.godotengine.org/en/stable/tutorials/scripting/c_sharp/c_sharp_basics.html), [UE Zen loader](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine), [UE remote build (forum)](https://forums.unrealengine.com/t/remote-build-from-windows-to-mac-doesnt-work/771179)

**Platform layer, scripting, content:**
- Platform layer: [SDL 3.4](https://www.phoronix.com/news/SDL-3.4-Released), [sdl3 crate](https://docs.rs/crate/sdl3/latest), [gilrs](https://gitlab.com/gilrs-project/gilrs), [cpal](https://github.com/RustAudio/cpal). crates.io API (2026-10-22) for winit, sdl3-sys, ash, vulkanalia, objc2-metal, sentry, minidumper, crash-handler, cargo-ndk, mlua, rquickjs, wasmi, wasmtime.
- Scripting: [rusty_v8 releases](https://github.com/denoland/rusty_v8/releases), [Wasmtime platforms](https://docs.wasmtime.dev/stability-platform-support.html), [Pulley](https://docs.wasmtime.dev/examples-pulley.html), [Wasmi 2.0 (secondary)](https://zeli.app/story/49521031), [Hermes V1](https://www.tothenew.com/blog/hermes-v1-by-default-in-react-native-0-84-the-biggest-performance-win-of-2026/), [Hermes AOT status](https://swmansion.com/blog/welcoming-the-next-generation-of-hermes-67ab5679e184), [Luau codegen](https://luau.org/2023/11/01/luau-recap-october-2023.html), [.NET NativeAOT](https://learn.microsoft.com/dotnet/maui/deployment/nativeaot)
- Content and device tooling: [Basis Universal v2.5](https://github.com/BinomialLLC/basis_universal), [iOS screenshot tooling (community)](https://tessl.io/registry/skills/github/NeverSight/learn-skills.dev/ios-device-screenshot), [WWDC26 metalperftrace (secondary)](https://daily.dev/posts/wwdc26-find-and-fix-performance-issues-in-your-metal-games-apple-focakni4v)
