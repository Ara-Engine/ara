# S7 RESULT: export and NativeAOT for Windows, Linux and Android

**Verdict: PASS on every criterion (2026-10-23).** A prebuilt Rust runtime template, C# gameplay
compiled ahead of time and a separate deterministic content pack ship and run on Windows, Linux and
Android. **All three are exported from the Windows machine.** Linux NativeAOT included: ILC runs on
Windows and Zig links against glibc 2.31. C# and Rust agree **bit for bit** on every lane, with
checksum `0x825f3722a803f55c` everywhere the full 120-tick scene ran. The Apple half (macOS, iOS) is a
separate spike (`spikes/s7-export-apple/`).

| Criterion | Result | Evidence |
|---|---|---|
| Windows NativeAOT builds and runs | **PASS** | `Game.dll` 1.82 MB, first C# call 7 ms after `main`, no JIT (`dynamicCode=False`) |
| Windows ReadyToRun + bundled CoreCLR builds and runs | **PASS** | 80.2 MB folder, loads an IL mod at runtime; 23.0 MB trimmed (mods then break) |
| Linux NativeAOT builds and runs in WSL | **PASS** | `Game.so` 1.86 MB; bit-exact; but needs GLIBC_2.34 and **fails on glibc 2.31** |
| Linux from Windows: is a Linux machine needed? | **Answered: no** | R2R (crossgen2) and NativeAOT (ILC + `zigcc`) both built on Windows; both run on **Debian 11 glibc 2.31** |
| Android APK/AAB build with the right ABIs | **PASS** | `arm64-v8a` + `x86_64`, 16 KB-aligned ELF and zip; no Gradle; APK 4.87 MB, AAB 2.29 MB, per-device download 1.10 MB (arm64) |
| Android runs on a device or emulator | **PASS (x86_64 emulator)** | NativeActivity APK and bundletool split APKs both run on API 37.2 with 16 KB pages; checksum in logcat. arm64 libs built and inspected, not executed (no arm64 device) |
| Pack is deterministic | **PASS** | byte-identical across 3 Windows builds, a moved/re-timestamped/reformatted source copy, and a Linux-host build |
| Doctor works | **PASS** | `ara-s7 doctor`: 23 real checks (more with `--devices`), NDJSON, stable codes, runnable fixes; caught 4 real issues on this machine |
| Lanes recommended with exact commands | **PASS** | [Recommendation](#recommendation-export-lanes), [Proposed `ara export` contract](#proposed-ara-export-contract) |

D11 holds for these three platforms: the S7 AOT gates pass. The Apple half decides the rest.

---

## What was built

```
spikes/s7-export/
  src/lib.rs, abi.rs, pack.rs, runtime.rs   runtime template core (crate ara_runtime)
  src/android.rs                            ANativeActivity_onCreate → same run loop (no Java, no dex)
  src/bin/ara-player.rs                     desktop runtime template (A6 "ara-player")
  src/bin/ara-s7/                           pack | pack-info | doctor | export | bench-start
  src/bin/zigcc.rs                          linker shim: zig cc pinned to x86_64-linux-gnu.2.31
  game/                                     C# gameplay: one source, AraLane=aot | r2r
  mod/                                      a plain IL mod (loads on the R2R lane only)
  content/                                  scene JSON + 64×64 TGA → cooked into the pack
  android/                                  AndroidManifest.xml (hasCode=false), BundleConfig.json
  scripts/                                  measure.sh, template-cold.sh, glibc231-check.sh, wsl-setup.sh, summarize.py
  results/                                  every recorded run (NDJSON event streams, logs)
```

1. **The game (C#, `game/`).** It uses the R10/S2 ABI shape:
   - `[UnmanagedCallersOnly(EntryPoint = "ara_script_init")] Init(HostApi*, ScriptApi*)` checks the ABI version and struct size, keeps the host table, and fills in `RunSystem`, `Checksum`, `Nop`, `CallHost` and `LoadMod`.
   - System 0 (`Movement`) runs gravity, integration and a ground bounce over **Rust-owned** `Transform`/`Velocity` columns. That's 100k entities in 7 chunks of 16k rows, with one boundary call per tick.
   - System 1 always throws. Every export catches it at the boundary, logs it through the host callback and returns status 1; the process survives.
   - `Checksum` (FNV-1a over the float bit patterns) logs through the host `log` callback. Rust runs an operation-identical reference system and demands an exact match.
   - `LoadMod` loads `mods/Mod.dll` via `AssemblyLoadContext`: it returns 0 on R2R and 2 on NativeAOT (`PlatformNotSupportedException`, caught).
2. **The runtime template** (`ara-player` on desktop, `libara_runtime.so` on Android) does five things in order:
   - opens the pack;
   - parses the scene (an A3-shaped flat entity map) and resolves `@textures/checker.tga`;
   - spawns 100k entities from the scene's seed (SplitMix64, no global RNG);
   - binds C#: on the NativeAOT lane it `dlopen`s the image and resolves `ara_script_init`; on the R2R lane it hosts CoreCLR directly (`coreclr_initialize` + `coreclr_create_delegate` on the same `[UnmanagedCallersOnly]` method, with no hostfxr);
   - runs 120 fixed ticks and prints one `ARA_RESULT {json}` line (logcat on Android).
3. **The pack (`.arapack` v1).** Layout: a 64-byte header, then a canonical-JSON manifest (sorted keys), then zstd-19 chunks, 4 KiB-aligned and ordered by hash.
   - Chunks are named by the BLAKE3 of their *cooked* bytes, and the pack id is the manifest's BLAKE3.
   - Cook steps: JSON is re-emitted canonically, so formatting edits don't change the pack. TGA is cooked to `ATEX` RGBA8.
   - The reader verifies the manifest hash and every chunk hash, and decodes with pure-Rust `ruzstd`, so the template needs no C toolchain per target.
   - On Android the pack is an *uncompressed* APK asset mapped with `AAsset_getBuffer` (logcat: `game.arapack 8364 bytes, mapped from APK`).
4. **The export driver (`ara-s7 export`)** runs these stages: `template → script → pack → assemble | package-apk + package-aab → sign → audit (Linux) → verify`.
   - Each stage emits NDJSON `progress`/`stage` events, or a coded `error` with fixes. Tool output goes to `out/logs/<lane>-<stage>.log`.
   - Verify runs the exported build: locally on Windows, inside WSL for Linux, and on Android via `adb install` + `am start` + logcat.
5. **The doctor (`ara-s7 doctor`)** checks every prerequisite above (see [Doctor](#doctor)).

## How to run it

Prerequisites: Rust 1.99.1, .NET SDK 10.0.x and MSVC build tools on Windows. For Linux: WSL `Ubuntu-24.04`, plus Zig for the Windows-hosted lanes. For Android: SDK, NDK r27+, JDK 17 and bundletool. `doctor` tells you what is missing, with fixes.

```sh
cd spikes/s7-export
export CARGO_BUILD_JOBS=4
cargo build --release --features tools --bins           # ara-player, ara-s7, zigcc
# one-time downloads into the ignored tools/ dir
curl -L -o tools/bundletool.jar https://github.com/google/bundletool/releases/download/1.18.3/bundletool-all-1.18.3.jar
curl -L -o zig.zip https://ziglang.org/download/0.17.0/zig-x86_64-windows-0.17.0.zip && unzip zig.zip -d tools && rm zig.zip
# WSL toolchains (only for the WSL lane, Linux benches and the glibc check)
wsl -d Ubuntu-24.04 -u root -e apt-get install -y clang lld zlib1g-dev
wsl -d Ubuntu-24.04 -e bash scripts/wsl-setup.sh

./target/release/ara-s7 doctor                                  # add --json for NDJSON, --devices for adb
./target/release/ara-s7 export --platform windows --lane aot    # → dist/windows-aot/
./target/release/ara-s7 export --platform windows --lane r2r    # → dist/windows-r2r/
./target/release/ara-s7 export --platform linux --lane aot-cross   # NativeAOT built on Windows (recommended)
./target/release/ara-s7 export --platform linux --lane r2r         # R2R built on Windows
./target/release/ara-s7 export --platform linux --lane aot         # NativeAOT built inside WSL (comparison)
./target/release/ara-s7 export --platform android                  # APK + AAB; verify runs if adb sees a device
dist/windows-aot/ara-player.exe --mod mods/Mod.dll --bench 20000000

bash scripts/measure.sh            # doctor, cold+warm exports, start benches, cross-host pack check
bash scripts/template-cold.sh      # cold template builds per target
wsl -d Ubuntu-24.04 -e bash scripts/glibc231-check.sh dist/linux-aot-cross dist/linux-r2r dist/linux-aot
python -I scripts/summarize.py     # the tables below, from results/*.ndjson
cargo test --release --features tools --lib                     # pack determinism + corruption tests
./target/release/ara-s7 pack content a.arapack && ./target/release/ara-s7 pack content b.arapack && cmp a.arapack b.arapack
```

This run left the following in WSL (it is stopped again):
- apt `clang lld llvm`;
- `~/.dotnet` (SDK 10.0.202) and `~/.cargo` (Rust 1.99.1);
- `~/ara-s7`, the Linux lane's synced build copy.

**Android run on the emulator.** No phone was attached. The installed system image (API 37.2 `google_apis_playstore_ps16k` x86_64, emulator 37.1.11) crash-loops surfaceflinger out of the box (see gotcha 9). These are the exact steps that worked:

```sh
avdmanager create avd -n ara_s7 -k "system-images;android-37.2;google_apis_playstore_ps16k;x86_64"
# only the SDK's adb on PATH (a second adb version breaks installs and auth)
emulator -avd ara_s7 -no-audio -no-boot-anim -no-snapshot -gpu host -memory 3072
adb shell setprop debug.sf.luma_sampling 0          # stops the surfaceflinger RegionSampling abort loop
./target/release/ara-s7 export --platform android    # install → am start → logcat ARA_RESULT
java -jar tools/bundletool.jar install-apks --apks=out/android-aot/ara-s7.apks   # AAB path
avdmanager delete avd -n ara_s7; adb kill-server
```

## Measurements

**Machine:**
- Hardware: AMD Ryzen 7 7800X3D (8C/16T), 31 GB RAM, RTX 4070 SUPER.
- OS: Windows 11 Pro 10.0.26300.
- Toolchains: Rust 1.99.1; .NET SDK 10.0.202 (runtime/ILCompiler 10.0.6); MSVC 14.50 (VS Build Tools 18.5).
- Android: NDK 28.2.13676358, build-tools 35.0.0, platform android-36, JDK 17.0.12, bundletool 1.18.3.
- Linux toolchain: Zig 0.17.0. WSL2 Ubuntu 24.04: kernel 6.6.87.2, glibc 2.39, clang 18.1.3.

`CARGO_BUILD_JOBS=4`, and other agents were building concurrently, so **timings are noisy (up to about 2×)**. Ranges are given where repeats were taken. Raw data is in `results/`.

### Export time

*Cold*:
- C# `obj/bin`, the lane's `out/` and `dist/` are deleted first.
- The NuGet cache is warm. The very first export per RID downloads its ILCompiler or runtime pack, which was not measured.

The numbers exclude the template stage: in Ara the template is a hash-pinned download. Template cold builds are listed separately.

| Lane | Built on | Cold export | Warm export | of which C# (`script`) cold / warm |
|---|---|---|---|---|
| windows aot | Windows | 17.4 s | 4.6–11.4 s | 17.4 / 4.5–11.4 s |
| windows r2r | Windows | 8.1 s | 2.7 s | 7.9 / 2.5 s |
| linux aot-cross | Windows (ILC + zigcc) | 12.6 s | 3.9–9.5 s | 12.6 / 3.9–9.5 s |
| linux r2r | Windows (crossgen2) | 7.8 s | 2.9 s | 7.6 / 2.7 s |
| linux aot | WSL (clang 18) | 5.4 s | 2.7 s | 5.4 / 2.7 s |
| android aot (2 ABIs, APK + AAB) | Windows (ILC + NDK clang) | 37.8 s | 11.0–18.2 s | 30.2 / 5.1–16.0 s |

The Android package stages take APK 1.2–4.5 s and AAB 3.9–6.0 s. Verify takes 0.2–0.5 s on desktop and 2–15 s on Android (install plus logcat polling).

**Template cold builds** (fresh target dir, dependencies included; `results/template-cold.txt`):
- windows-x64: 15.5 s
- linux-x64 via zigcc: 13.7 s
- android arm64: 12.0 s
- android x86_64: 14.3 s

### Output size (the C# part separated out)

| Lane | Template | C# part | Pack | Total |
|---|---|---|---|---|
| windows aot | 463 KB | **1.82 MB** (`Game.dll`) | 8.4 KB | 2.29 MB |
| windows r2r | 463 KB | **80.2 MB** (CoreCLR + framework + R2R `Game.dll`); 23.0 MB trimmed | 8.4 KB | 80.7 MB |
| linux aot-cross | 676 KB | 1.86 MB (`Game.so`) | 8.4 KB | 2.55 MB |
| linux aot (WSL) | 653 KB | 1.86 MB | 8.4 KB | 2.53 MB |
| linux r2r | 676 KB | 82.6 MB | 8.4 KB | 83.3 MB |
| android arm64-v8a | 551 KB | 1.75 MB (`libgame.so`) | 8.4 KB | **1.10 MB per-device download** |
| android x86_64 | 653 KB | 1.86 MB | 8.4 KB | 1.20 MB per-device download |
| android APK (both ABIs, stored) / AAB | | | | 4.87 MB / 2.29 MB |

The NativeAOT image costs about 1.8 MB for this small library, including reflection and `AssemblyLoadContext` code paths. The template (Rust: pack reader, ruzstd, BLAKE3, serde_json, libloading) costs 0.46–0.68 MB.

### Cold start to the first C# call

`spawn → first C# call` runs from the parent's spawn timestamp (`ARA_T0_UNIX_NS`) to `ara_script_init` returning, in `bench-start`'s 11 spawns. The first spawn is the coldest the OS allows without admin rights. Run in-process, the 100k-entity spawn is about 2–3 ms of it.

| Lane | First spawn | Warm median | Script image load (warm) | Process wall (warm) |
|---|---|---|---|---|
| windows aot | 75.4 ms | 27.1 ms (`main` → first call: ~7 ms) | 4.7 ms | 31.4 ms |
| windows r2r | 131.5 ms | 63.1 ms (`main` → first call: ~40 ms) | 39.1 ms (CoreCLR init) | 69.9 ms |
| linux aot (WSL-built) | 4.1 ms | 4.4 ms | 0.2 ms | 6.2 ms |
| linux aot-cross (Windows-built) | 17.3 ms | 4.2 ms | 0.2 ms | 5.8 ms |
| linux r2r | 122.6 ms | 18.3 ms | 12.4 ms | 22.9 ms |
| android x86_64 emulator, `adb shell` | | 28.0 ms from `main` (spawn 17.8, image load 0.8) | | |
| android x86_64 emulator, NativeActivity | | 12–113 ms from `onCreate` (6 samples); `am start` TotalTime 0.9–2.8 s | | |

About 20 ms of the Windows numbers is process creation plus Defender, before `main` runs. It isn't C#. The C# cost of startup is the image-load column: NativeAOT **4.7 ms** on Windows and **0.2 ms** on Linux, versus CoreCLR **39 / 12 ms**. Emulator numbers are indicative only.

### Rust ↔ C# call overhead and the system tick

| Lane | Rust→C# | C#→Rust | C#→Rust `[SuppressGCTransition]` | C# `Movement`, 100k (best tick) | Rust reference |
|---|---|---|---|---|---|
| windows aot | **3.60 ns** | **3.54 ns** | **1.09 ns** | 0.170 ms (1.70 ns/entity) | 0.132 ms (1.32 ns/entity) |
| windows r2r | 4.15 ns | 3.75 ns | 1.14 ns | 0.188 ms (1.88) | 0.131 ms (1.31) |
| linux aot (WSL VM) | 5.49 ns | 5.31 ns | 1.74 ns | 0.171 ms (1.71) | 0.125 ms (1.25) |
| linux aot-cross | 6.29 ns | 5.45 ns | 2.10 ns | 0.174 ms (1.74) | 0.129 ms (1.29) |
| linux r2r | 5.90 ns | 6.06 ns | 1.77 ns | 0.218 ms (2.18) | 0.136 ms (1.36) |
| android x86_64 emulator | 11.45 ns | 22.35 ns | 2.99 ns | 0.181 ms (1.81) | 0.150 ms (1.50) |

Under NativeAOT the boundary costs the same as R10's JIT measurement (3.6 / 3.3 / 1.1 ns), so the ABI rule stays "one call per chunk batch, never per entity". The C# system runs at 1.3× the Rust loop on every lane. On the emulator, bench numbers measured inside the NativeActivity process were noisier (47 / 33 / 7 ns).

### Determinism

- **Pack:** sha256 `885e930e…`, file BLAKE3 `3bad8b0b…`, pack id `c7d22561…`. It was identical across:
  - two consecutive builds;
  - a copy at another path, with every mtime changed and the scene JSON re-indented;
  - the Linux-built `ara-s7` in WSL (`results/pack-determinism.ndjson`).

  `cargo test` covers determinism, round-trip and corruption detection (a chunk flip gives `ChunkHash`, a manifest flip gives `ManifestHash`). The `mobile` profile pack has a different id (`302568c6…`) because the profile is in the manifest. That is intended: cook keys include the profile.
- **Simulation:** after 120 ticks the checksum is `0x825f3722a803f55c` for:
  - C# NativeAOT on win-x64, linux-x64 (both builds) and linux-bionic-x64;
  - C# R2R/CoreCLR on win-x64 and linux-x64;
  - the Rust reference on all of them.

  RyuJIT (JIT and AOT) does not contract `a*b+c` into FMA, and neither does rustc. arm64 was not executed.

## Findings and gotchas

1. **NativeAOT cross-compiles from Windows to Android.**
   - Flags: `dotnet publish -r linux-bionic-arm64 -p:DisableUnsupportedError=true -p:PublishAotUsingRuntimePack=true`, with the NDK's `toolchains/llvm/prebuilt/windows-x86_64/bin` on PATH. That's it.
   - The .NET 10 targets already handle a Windows host: `where /Q` probing, `--target=aarch64-linux-android21`, `-fuse-ld=lld`, and `-Wl,-z,max-page-size=16384` for bionic.
   - The output needs only `libc`/`libm`/`libdl`. "Experimental" did not bite for a NativeActivity app that never touches Java.
2. **NativeAOT also cross-compiles from Windows to Linux.** ILC runs on Windows and `zigcc` (a ~110-line shim around `zig cc`, same idea as PublishAotCross) does the link:
   - `-p:DisableUnsupportedError=true -p:CppCompilerAndLinker=zigcc -p:LinkerFlavor=lld -p:ObjCopyName=llvm-objcopy` (NDK `llvm-objcopy`).
   - The shim pins `x86_64-linux-gnu.2.31`, so the result needs GLIBC_2.29. It runs under Debian 11's glibc 2.31 loader, as does the Zig-linked Rust template (GLIBC_2.30).
3. **The obvious Linux route is the wrong one.** WSL-built binaries need GLIBC_2.32–2.34. `scripts/glibc231-check.sh` shows glibc 2.31 rejecting both the WSL template and the WSL `Game.so`. Microsoft's own CoreCLR `.so` files need only ≤2.27 (+ `libstdc++` from GCC 10). Only *our* binaries need the pinned sysroot.
4. **R2R to Linux from Windows just works.** crossgen2 cross-targets, and the linux-x64 runtime pack is a NuGet download. The C# part of a Linux R2R export never needs Linux.
5. **Direct CoreCLR hosting works for self-contained R2R** (`coreclr_initialize` + `coreclr_create_delegate` on an `[UnmanagedCallersOnly]` method; no hostfxr, no `runtimeconfig` resolution). Three caveats:
   - You must pass `RUNTIME_IDENTIFIER` yourself, or `RuntimeInformation.RuntimeIdentifier` says `unknown`.
   - Self-contained publishing insists on an apphost (NETSDK1067), which the exporter drops.
   - Trimming removes `Exports.Init` (TypeLoad `0x80131522`) unless the game assembly is rooted. NativeAOT roots `EntryPoint` exports by itself.
6. **Mods versus size.** Untrimmed R2R (80 MB) loads IL mods. Trimmed R2R (23 MB) hosts fine, but the mod fails on a trimmed framework member. A moddable lane must ship either the full framework or a rooted, whitelisted mod API surface (the s&box-style whitelist from R10 becomes the trimmer root descriptor). NativeAOT never loads mods.
7. **Windows NativeAOT needs `vswhere.exe` on PATH.** `findvcvarsall.bat` calls it, and without it the link fails with MSB3073. The exporter prepends it and the doctor reports it.
8. **Android without Gradle is about 6 tool calls.**
   - APK: `aapt2 link` (manifest only, `-I android.jar`) → `jar xf`, add `lib/<abi>/*.so` + `assets/` → `jar cfM0` (stored) → `zipalign -P 16 -f 4` → `apksigner`.
   - AAB: the same with `--proto-format` → module zip → `bundletool build-bundle` (with `uncompressedGlob` for the pack) → `jarsigner`.
   - `android:hasCode="false"` plus `android.app.NativeActivity` means zero dex and zero Java.
   - APK/AAB bytes are **not** reproducible yet, because `jar` writes mtimes. Ara's exporter should write the zips itself, with fixed order and timestamps.
9. **The emulator was the hardest part, and none of it was our app's fault.**
   - **SurfaceFlinger abort loop.** Emulator 37.1.11 with the API 37.2 ps16k image aborts in `GoldfishMapper::readFromHost` (`!hasReadColorBufferDma`) from SurfaceFlinger's RegionSampling thread. That happens under SwiftShader and host GPU, windowed or headless. system_server then restarts every ~10 s, PackageManager never persists an install ("Missing permission state"), and `am start` reports "Activity class … does not exist". `adb shell setprop debug.sf.luma_sampling 0` fixes it.
   - **adb version fight.** The SDK's adb 37.0.1 and the adb 36.0.2 on PATH kill each other's server, which gives "Broken pipe" installs and `unauthorized` after a wipe. The doctor now flags this (`ARA-EXP-AND-012`). Also use `adb install --no-incremental`.
10. **`--disable-build-servers`** keeps `dotnet` from leaving MSBuild/VBCSCompiler processes behind, at a cost of a few seconds on warm exports. ILC itself isn't incremental, so a warm NativeAOT export is still 4–11 s.
11. **Startup on Windows** is dominated by process creation plus Defender (about 20 ms) and, for R2R, CoreCLR init (about 40 ms). NativeAOT's own load is about 5 ms.

## Recommendation: export lanes

**One driver, a template lane by default.** An export never compiles Rust. It does compile C#, because AOT is per target. **Windows hosts every non-Apple export.**

| Platform | Default lane | Alternative | Needs on the export machine |
|---|---|---|---|
| Windows x64 | **NativeAOT**: `ara-player.exe` + `game/Game.dll` + `game.arapack` | **R2R + bundled CoreCLR** for moddable games (untrimmed 80 MB, or trimmed with a rooted mod API) | .NET SDK, MSVC build tools (AOT only) |
| Linux x64 | **NativeAOT via ILC + `zigcc`, glibc 2.31**, built on Windows | R2R via crossgen2, built on Windows (no native toolchain at all) | .NET SDK, Zig (AOT only); WSL/container only to *verify* |
| Android arm64 (+ x86_64 for emulators) | **NativeAOT linux-bionic via the NDK clang**; NativeActivity; APK for sideload/dev, AAB for Play; built on Windows | CoreCLR or Mono runtime packs exist for .NET 10 (`microsoft.netcore.app.runtime.android-arm64`, `.mono.android-arm64`); keep them as plan B, not needed today | .NET SDK, NDK r27+, build-tools, JDK 17, bundletool |

Exact commands per lane are the ones `ara-s7 export` runs (see `out/logs/*.log`):

```sh
# Windows NativeAOT (vswhere dir on PATH)
dotnet publish game/Game.csproj -c Release -r win-x64 -p:AraLane=aot --disable-build-servers -o out/windows-aot/script
# Windows / Linux R2R + CoreCLR
dotnet publish game/Game.csproj -c Release -r win-x64|linux-x64 -p:AraLane=r2r --disable-build-servers -o out/<lane>/script
# Linux NativeAOT from Windows (zigcc dir + NDK llvm bin on PATH)
dotnet publish game/Game.csproj -c Release -r linux-x64 -p:AraLane=aot -p:DisableUnsupportedError=true -p:CppCompilerAndLinker=zigcc -p:LinkerFlavor=lld -p:ObjCopyName=llvm-objcopy -o out/linux-aot-cross/script
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=target/release/zigcc.exe cargo build --release --bin ara-player --target x86_64-unknown-linux-gnu   # template (CI)
# Android NativeAOT from Windows (NDK llvm bin on PATH)
dotnet publish game/Game.csproj -c Release -r linux-bionic-arm64 -p:AraLane=aot -p:DisableUnsupportedError=true -p:PublishAotUsingRuntimePack=true -o out/android-aot/script/arm64-v8a
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=<ndk>/…/bin/clang.exe CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=--target=aarch64-linux-android29 -C link-arg=-Wl,-z,max-page-size=16384" cargo build --release --lib --target aarch64-linux-android   # template (CI)
aapt2 link -o base.apk --manifest android/AndroidManifest.xml -I <sdk>/platforms/android-36/android.jar --min-sdk-version 29 --target-sdk-version 36 [--proto-format]
jar cfM0 unaligned.apk -C staging . && zipalign -P 16 -f 4 unaligned.apk aligned.apk && apksigner sign --ks <ks> --out ara-s7.apk aligned.apk
java -jar bundletool.jar build-bundle --modules=base.zip --output=ara-s7.aab --config=android/BundleConfig.json && jarsigner -keystore <ks> ara-s7.aab <alias>
```

Supporting rules:
- **Linux templates (and Linux NativeAOT images) are always linked against a pinned glibc 2.31 sysroot.** That's Zig here; in CI it's Zig or the sniper SDK container. Never against whatever distro the builder runs.
- The Android floor is API 29 and the target API is 36. Every `.so` is linked with `-z max-page-size=16384` and the zip is page-aligned with `zipalign -P 16`. The pack is stored uncompressed, so it is mapped instead of copied (AAB: `uncompressedGlob`).
- Device-side development builds keep using NativeAOT. A JIT dev runtime on Android is not needed for the template lane.
- The milestone S7 criterion says "signed builds launch". Android builds were signed (a generated debug key) and launched. Windows signing was documented, not done, because no certificate was available ([Windows signing](#windows-signing)). Linux has no signing step.

## Doctor

`ara-s7 doctor [--platform P] [--devices] [--json]` makes read-only checks. It leaves WSL stopped if it found it stopped, and kills the adb server only if it started it. It exits 1 on any `error` and 0 otherwise. On this machine: 0 errors, 4 warnings (`results/doctor.txt`, `results/doctor.ndjson`).

| Area | Checks |
|---|---|
| Common | .NET 10 SDK (`DOT-001`); Rust 1.99.1 pin (`RS-001`) |
| Windows | MSVC VC tools via vswhere (`WIN-001`); vswhere on PATH (handled by export); ILCompiler win-x64 cached (`WIN-003`); Authenticode identity + signtool (`WIN-010`, warn) |
| Linux | WSL distro and state (`LNX-001`); bash probe (`LNX-002`); .NET in WSL (`LNX-003`); clang + zlib (`LNX-004`); Rust in WSL (`LNX-005`); Zig for the Windows-hosted lanes (`LNX-006`); WSL glibc vs the 2.31 baseline (`LNX-010`, warn); linux-x64 runtime pack cached (`LNX-011`) |
| Android | SDK (`AND-001`); NDK ≥ r27 with host clang (`AND-002`); aapt2/zipalign/apksigner (`AND-003`); android-36 platform (`AND-004`); JDK ≥ 17 (`AND-005`); bundletool (`AND-006`, warn); upload keystore (`AND-007`, warn); Rust targets (`AND-008`); bionic NativeAOT pack cached (`AND-010`); adb version conflict (`AND-012`, warn); with `--devices`: attached devices or AVDs to start (`AND-009`, `AND-011`) |

## Proposed `ara export` contract

```
ara doctor [--platform windows|linux|macos|android|ios] [--devices] [--json]
ara export --platform P [--lane aot|r2r] [--profile debug|profile|release] [--out DIR] [--clean] [--json]
ara run    --platform P [--device ID] [--scenario FILE] [--capture screenshot,perf:10s] [--json]
ara device list|logs|install|pull|screenshot [--device ID] [--json]
```

- **Exit codes:** 0 ok; 1 failed (at least one `error` event was emitted first); 2 usage.
- **Stream:** with `--json`, stdout carries one NDJSON object per line. Human mode prints the same events as text. Every event has `type` and `t_ms`.

| `type` | Fields | Meaning |
|---|---|---|
| `progress` | `stage`, `message` | stage started / intermediate progress |
| `stage` | `stage`, `ms`, `detail` | stage finished |
| `artifact` | `kind`, `path`, `bytes`, `detail` (`pack_id`, `file_blake3`, …) | produced file |
| `check` | `id`, `status` (`ok\|warn\|error`), `code`, `message`, `fixes`, `context` | doctor check |
| `warning` | `code`, `stage`, `message`, `fixes`, `context` | non-fatal, the export continues |
| `error` | `code`, `stage`, `message`, `fixes`, `context` (always includes `log`) | fatal |
| `result` | `result`: `stages[]`, `sizes`, `verify{pass, report}`, `total_ms`, … | final summary |

- **Fixes** are `{"kind":"command|env|doc","run":"…"}`, and `command` fixes run as-is.
- **Codes:** `ARA-EXP-<AREA>-<NNN>`, with areas `CLI DOT RS WIN LNX MAC IOS AND PACK VER`.
  - `0xx` covers prerequisites and warnings: 000–009 missing tools, 010+ degraded output such as unsigned or glibc.
  - `1xx` covers stage failures: 101 template, 102 script, 104 assemble/package, 105 audit/package-aab, 106 verify.
- **Stages:** `resolve → template → script → pack → assemble|package-* → sign → audit → verify`. `ara run` is `verify` against a chosen device. It streams logs and exits on the build's own result line. In the spike that line is `ARA_RESULT`; in Ara it is ara-bridge.

Real examples (`results/error-examples.ndjson`, `results/export-*.ndjson`):

```json
{"code":"ARA-EXP-LNX-001","context":{},"fixes":[{"kind":"command","run":"wsl --install -d Debian-13"},{"kind":"env","run":"ARA_WSL_DISTRO=<name>"}],"id":"linux.wsl","message":"WSL distro Debian-13 not found; the NativeAOT Linux lane builds inside it","status":"error","t_ms":123.2,"type":"check"}
{"code":"ARA-EXP-LNX-010","context":{"files":[["ara-player","GLIBC_2.34"],["game/Game.so","GLIBC_2.34"]]},"fixes":[{"kind":"doc","run":"Build Linux templates (and NativeAOT images) against a glibc 2.31 sysroot: sniper SDK container, or zig cc -target x86_64-linux-gnu.2.31"}],"message":"needs glibc 2.34; Steam Runtime 3 (sniper) provides 2.31","stage":"audit","t_ms":24329.7,"type":"warning"}
{"code":"ARA-EXP-CLI-002","context":{},"fixes":[{"kind":"doc","run":"windows: aot|r2r, linux: aot|aot-cross|r2r, android: aot"}],"message":"no lane r2r for platform android","stage":"resolve","t_ms":0.0,"type":"error"}
```

## Windows signing

The exporter emits `ARA-EXP-WIN-010` (unsigned) with the fix. Every PE in the export is signed: `ara-player.exe`, `game/Game.dll`, and on the R2R lane every `dotnet/*.dll` and `*.exe`. Each gets SHA-256 and an RFC 3161 timestamp:

```
signtool sign /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 ^
  /dlib Azure.CodeSigning.Dlib.dll /dmdf metadata.json dist\windows-aot\ara-player.exe dist\windows-aot\game\Game.dll
```

The default identity is **Azure Artifact Signing** (about $10/month; individuals in the US or Canada only). An OV certificate in a cloud HSM is the alternative. SmartScreen reputation still has to accrue, and EV no longer bypasses it (R08). Steam and itch don't require signing. Sign *after* assemble and *before* the pack is hashed into any manifest. The pack is data and is never Authenticode-signed; its integrity comes from the manifest hash.

## What's left

- **Not done here:**
  - Running arm64 Android on hardware (the libs are built, 16 KB-aligned and in the AAB split).
  - A real Steam Runtime sniper container run (a Debian 11 glibc 2.31 loader stood in for it).
  - A real Authenticode signature, and the Play upload key flow.
- **Missing exporter features:**
  - Reproducible APK/AAB zips.
  - Symbol archiving: `Game.pdb` and `.dbg` by build id.
  - Play Asset Delivery split above 200 MB.
  - ara-bridge in dev builds instead of `ARA_RESULT` lines.
- **Not yet checked:** static linking (`NativeLib=Static`) of the C# image into one template binary. iOS needs it; the desktop and Android `dlopen` model is fine as is.
- **Watch:**
  - .NET 10.0.13 is out while this machine is on 10.0.6 (SDK 10.0.202). Pin the SDK per engine release, as D17 does for Rust.
  - .NET 11 (November 2026) defaults Android to CoreCLR; re-check bionic NativeAOT's status then.

## Proposed decision

> **S7 (Windows / Linux / Android half): PASS (2026-10-23). Export lanes adopted; D11 holds for these platforms.**
> - **Model:** a hash-pinned, prebuilt Rust runtime template, plus a per-target AOT C# image, plus a deterministic content-addressed `.arapack`. **Windows hosts every non-Apple export.** An export compiles C#, never Rust.
> - **Windows:** NativeAOT by default (`Game.dll` 1.8 MB; first C# call ~5 ms after image load). R2R + bundled CoreCLR is the opt-in moddable lane: 80 MB untrimmed, or trimmed with a rooted mod-API surface.
> - **Linux:** built on Windows. NativeAOT via ILC + a Zig linker pinned to **glibc 2.31**; R2R via crossgen2. Never link Linux binaries against the build host's glibc: Ubuntu 24.04 builds need 2.34 and fail on sniper. WSL or containers are for verification only.
> - **Android:** NativeAOT `linux-bionic` (arm64 + x86_64) from Windows with the NDK clang. `NativeActivity` with no Java. APK/AAB via aapt2 + bundletool, no Gradle. API 29 floor, target API 36, 16 KB-aligned. The CoreCLR/Mono runtime packs stay as plan B.
> - **Boundary:** the R10 ABI is unchanged under AOT (3.6 / 3.5 / 1.1 ns); exceptions are contained at the boundary on every lane; C# and Rust are bit-identical across all lanes and OSes.
> - **Tooling:** `ara doctor` / `export` / `run` emit NDJSON events with stable `ARA-EXP-*` codes and runnable fixes, as specified in `spikes/s7-export/RESULT.md`.
