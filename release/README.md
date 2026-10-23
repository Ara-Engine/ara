# Engine release tooling

Run on Windows x64 with Rust/MSVC, Vulkan SDK and the x64 .NET 10 SDK installed.
Set `CARGO_BUILD_JOBS=2` and `VULKAN_SDK` to the SDK being redistributed.

```powershell
cargo run -p ara-release -- package --version 0.0.1-alpha
cargo run -p ara-release -- notes --version 0.0.1-alpha
cargo run -p ara-release -- smoke
```

These commands do not tag, push, publish, or contact a crash-reporting service.
The packager explicitly builds release binaries with matching line-table PDBs.
`target/release-dist` holds the distribution, ZIP, self-extracting setup EXE,
private symbols ZIP, notes, BLAKE3/size inventory, build logs and smoke evidence.
An existing completed distribution is never overwritten; move it before repacking.
The source commit is recorded in `engine.json`; runtime file hashes in `files.json`.

The SFX reuses `ara-export-installer`'s archive validation and atomic installation.
`--silent --destination <new path>` supports automated install. The portable ZIP
also contains `install.ps1`, which stages a new installation before registration.
`-RegisterOnly` completes shell/Hub registration of an already extracted directory.
Default destination: `%LOCALAPPDATA%\Programs\Ara\<version>`. No administrator,
WiX, NSIS, CMake or Python is needed. An exact version is registered through Hub
RPC; projects and other installed versions remain separate. Versioned Start menu
shortcuts, a user PATH entry, Installed apps registration and `uninstall.cmd` are
created. Uninstall deletes only installed files and empty folders, and refuses to
remove an engine with an open registered project. Projects, settings and crashes
survive. Smoke disables real-user shell integration and records that distinction.

Slang, supporting DLLs/standard modules, DXC and spirv-val come from the Vulkan SDK.
DXC is required because the current cooker emits DXIL alongside SPIR-V. Actual
upstream licenses and dependency notices are in `licenses/`; SDK Slang 2026.17
uses Apache-2.0 WITH LLVM-exception, not historical MIT. `licenses/SOURCES.md`
records sources. Tool discovery prefers bundled tools, then development SDKs.
Cooked resources load relative to the installed executable, never the checkout.

The .NET SDK/runtime/reference packs are prerequisites, never copied from Program
Files. Ara.Host, Ara.Runtime, compile server, analyzers, generators and agent worker
are framework-dependent. Roslyn/NuGet dependencies are open-source libraries;
their package manifests, license expressions and notices are aggregated. This
avoids redistributing a proprietary SDK or runtime installer. Doctor and setup
print the official .NET 10 download link if missing. NativeAOT game export has
separate developer prerequisites, outside engine startup.
The SDK's shader-tool PE imports also require Microsoft's x64 VC++ v14 runtime
(`msvcp140.dll`, `vcruntime140.dll`, `vcruntime140_1.dll`). It is an external
prerequisite; setup checks it before invoking native tools and links Microsoft's
supported download page. No proprietary CRT DLL is copied out of System32 or an SDK.

All Cargo metadata packages (including transitive/build/dev/other-target packages)
are inventoried with SPDX expressions and original license/notice texts, along
with vendored SDL3, Jolt, Recast/Detour, libopus, fonts/icons, FSR, meshoptimizer.
The deliberately inclusive inventory does not claim every package is linked.
When a published crate omits its license files, cached upstream notices and full
SPDX texts supplement its metadata attribution. Exact upstream revisions are
recorded alongside those notices. New unsupported SPDX IDs fail packaging until
their texts are added. Every third-party package has a source-download link,
including the unmodified MPL-licensed audio dependencies.

Samples are copied when `samples/<id>/ara.project.json` exists; caches, build
directories and evidence are excluded. `samples/assets.lock.json` is copied beside
templates for the samples' first-open acquisition workflow. No sample source is
changed. `ara init` always supplies `templates/starter`.

The Hub app slot is `ara-hub.exe`, automatically packaged when the `ara-hub` crate
exists. Until then its Start menu launcher explains the slot and uses the Hub
service. Native Hub startup should handle `ara_crash::monitor_from_args` first,
retain `ara_crash::desktop::start("ara-hub", true)` and surface CARGO_PKG_VERSION.
The editor follows this path; the service captures silently. Reports stay under
`%LOCALAPPDATA%\Ara\crashes`; the next interactive editor launch offers to open
new reports through a native message box, outside the editor layout.

Optional Authenticode: put `signtool.exe` on PATH and set `ARA_SIGN_THUMBPRINT`.
Setup is signed after appending the payload. Explicitly configured signing
failures are fatal; without a certificate, the release records an unsigned alpha.

Keep the private symbols ZIP for the exact build, outside the engine archive.
Extract it and run `cargo run -p ara-release -- symbols <symbols-directory>` to
convert PDBs into a local Breakpad store. Then run
`cargo run -p ara-crash --features symbols -- symbolize <dump> <symbol-store>`.
Matching PE images accompany PDBs for Windows unwind information. No symbol server
or telemetry is contacted.
