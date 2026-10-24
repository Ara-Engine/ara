# Ara export integration

## Gameplay scripts in runtime exports

`playerTemplate: "runtime"` builds every gameplay `ara.module.toml` module into
one NativeAOT shared library using the real generated `ara_script_init` ABI
(A12/A13), rather than the S7 conformance ABI. References are linked in dependency
order; editor modules are excluded, and missing dependencies/cycles are errors.
The export job generates isolated projects with the SDK generator and gameplay
analyzers. It does not modify the game or require game-owned `.csproj` files.
Runtime presets may omit `scriptProject`; it remains required by conformance presets.
`ARA_DOTNET_SDK` selects the SDK's `dotnet/` sources, or
`ARA_DOTNET_ARTIFACTS` its prebuilt `bin/` tools; these are export-time inputs only.

The player folder contains `game/<assembly>.dll` (or `.so`) and
`game/scripts.json`: format, ABI version, engine layout hash and linked modules.
The packed `PlayScene` uses `ScriptScene::shipping("game/scripts.json")`; the
standalone entry resolves this path against the player folder. The runtime checks
metadata before loading and the native initializer independently checks compiled
bindings. `ARA0921` means rebuild scripts and player with the same Ara SDK and
export again; `ARA0920` reports missing images or an inconsistent module inventory.
Generation, runtime binding and export hashes use the same `engine_bindings()` set;
adding an authoring schema alone does not change the C# storage ABI.
Shipping loads no author paths, CoreCLR installation, Roslyn, watcher or hot reload.
Gameplay state remains ECS-owned and uses the usual snapshots and restore path.

This runtime lane uses NativeAOT, as required by A12. The existing optional
ReadyToRun and S7 fixture lanes remain available for conformance templates;
ReadyToRun is diagnosed for runtime gameplay exports until its host is integrated.

## Distribution v2

Export writes a plain `build/` folder, `distribution.json`, `report.json`, generic
packages, and a private `symbols/<build-id>.zip` plus its unpacked symbol store.
`distribution.json` lists player files and packages with BLAKE3, length and execute
permission, the entry point, build ID, symbol location and actual signing state.
Symbols, logs and crash records are excluded from every player package.

Stores will be supported by optional plugins later. The engine produces generic
builds and manifests; this change introduces no store integration or plugin system.

The preset's `packaging` field selects `windowsPortable`, `windowsInstaller`,
`linuxAppimage`, `linuxTar` or `androidPlay` (`folder` is the default). Generic
project presets live at `presets/<name>.export.json`; `export --preset
windows-portable --project PROJECT --output NEW_DIR` resolves that file. Paths to
existing presets still work. `ara-export preset --kind windows-portable --base
BASE --output PROJECT/presets/windows-portable.export.json` derives a named preset
from a platform preset with real runtime pins. The CLI owner can call the unchanged
`export(ExportRequest, EventSink)` entry point for an export job and forward its
progress, artifact and diagnostic events to `ara_jobs`.

- `windows-portable`: deterministic `game.zip`.
- `windows-installer`: `game.zip` and a self-extracting `game-setup.exe`. Prepare
  `ara-export-installer` once, pin it with `artifact-pin --file FILE`, and put that
  pin in `installerStub`. The installer supports `--list` and `--destination DIR`,
  installs into a new directory and never updates an existing installation.
  No WiX/NSIS, elevated process, service or system configuration is involved.
- `linux-tar`: deterministic `game.tar.gz`, with executable files marked 0755.
- `linux-appimage`: type-2 `game.AppImage`, built with Rust SquashFS/Gzip on Windows.
  Supply an x86_64 [AppImage runtime](https://github.com/AppImage/type2-runtime/releases)
  as a BLAKE3 `appimageRuntime` pin. Download it explicitly during setup; it is
  never fetched by Cargo. The ELF baseline remains glibc 2.31. FUSE/extraction and
  actual execution need Linux; Windows verifies the embedded image structure.
- `android-play`: requires release configuration and an explicit upload signing
  key; emits the existing signed APK/AAB with bundletool, signature and 16 KiB
  structure checks. A debug key cannot select this preset.

Optional `windowsSigning: { certificateEnv, timestampUrl }` signs player PE images
and the finished installer using the certificate store and SHA256. A configured
identity makes missing signtool/signing failures fatal; an absent identity yields
explicitly unsigned packages. `ARA_SIGNTOOL` overrides discovery. Doctor diagnoses
the tool and certificate reference without printing values. Actual identity/trust
and installation are not claimed by an unsigned export.

## Crash reporting v2

Desktop templates retain native debug information at preparation time:
`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`, `CARGO_PROFILE_RELEASE_STRIP=none`.
Windows template pins include `symbols: { hash, source }` for the matching PDB;
`pin --symbols PDB` produces it. Export checks the PE's GUID/age against the PDB.
Linux/Android templates carry DWARF. C# NativeAOT is published with symbols retained.
Export archives raw PDB/DWARF and a standard Breakpad symbol store, then strips
the deployed ELF files. `ARA_OBJCOPY` or the installed LLVM/NDK supplies objcopy;
doctor diagnoses setup, and Cargo never invokes it to build these crates.

`ara-crash` uses [crash-handler and minidumper](https://github.com/EmbarkStudios/crash-handling)
to request a native dump from an isolated worker: MiniDumpWriteDump on Windows,
ptrace and Breakpad-format minidumps on Linux. The same executable has a private
monitor mode; production templates must call `monitor_from_args` before runtime
initialization and retain the `Capture` guard. The export conformance template does
this. Native callbacks only perform preconnected IPC. A normal exit detaches the
handler and reaps the worker. Forced termination/fast-fail paths that bypass native
handlers cannot guarantee capture. Android's native capture is not implemented;
the managed reporting import remains available there.

Rust panic hooks record the message/source and a forced backtrace, plus a paired
native snapshot for offline symbolization after PDB/DWARF is stripped. A caught
panic keeps its fatal crash connection available for later faults. A compile-time
managed module initializer attaches `AppDomain.UnhandledException` and records the
exception's full managed stack before termination. Export injects it without editing
the game project or changing the host ABI. Its bounded `ARAMCS1` record is converted
by Rust's canonical writer during symbolization. Contained C# boundary exceptions
continue to use the existing host diagnostics. Crash storage is
`LOCALAPPDATA/Ara/crashes/<build-id>` or `XDG_STATE_HOME/Ara/crashes/<build-id>`
(fallback `~/.local/state`); `ARA_CRASH_DIR` is an explicit test override.

`ara-export crash symbolize DUMP --symbols EXPORT/symbols/BUILD/store` processes
native `.dmp`, Rust `.crash.json` and managed `.managed` files offline and emits
both the report and diagnostic `ARA2701`. `ara-crash symbolize DUMP SYMBOL_DIR`
is a standalone tooling binary. No symbol server is contacted. The integrating CLI
owner wires `ara crash symbolize` to `ara_crash::symbols::symbolize`; the host
registers `ara_crash::why::CrashExplainer` through `Host::why().register(...)` and
ingests the emitted diagnostic through its normal bus/report seam. The explainer
quotes the exception and stack, and requires the matching build's symbols; it
does not infer a root cause from a stack alone.

`crash: { uploadEndpoint, consentText }` optionally configures an HTTPS receiver.
Defaults disable upload. Capture/startup never uploads; the game must show the
configured disclosure and call `upload(..., UploadConsent::Granted, artifact)`
after consent for that attempt. Decline or absent configuration starts no network
operation. Upload uses optional OS curl with a 30-second deadline and a 128 MiB cap.

`playerTemplate` is the explicit template-to-runtime switch. It defaults to
`conformance` and emits launch/report contract `ara.player.conformance/1`. After
the runtime owner provides integrated player binaries and matching symbols, replace
the platform pins and set `playerTemplate: runtime` (`ara.player.runtime/1`). The
conformance executable rejects that contract, so it cannot stand in for an engine
player. The runtime template must implement the launch JSON fields, load the pack's
`ara.project.json` main scene and cooked assets, use the generated C# host ABI, call
the crash monitor/startup seam, and emit an honest headless `ARA_RESULT` including
`template_contract: "ara.player.runtime/1"` and `verified: true`. Windows export
rejects a result whose contract differs from the selected template.

The conformance fixture can validate a pack containing the real Fox demo project:
it verifies every file and the main scene document, then runs the original seeded
S7 ECS/C# workload. Its report explicitly says conformance; it does not simulate
the authored Fox controllers, physics, navigation or rendering. Stage a copy of
`crates/ara-player/demo` under a scratch project's `content/` and the existing
`fixtures/sample/game` under `game/`; this is export-test scaffolding. Never change
the demo or label this fixture as a runnable Outpost game. The Outpost Door,
UseMedkit and Gravity samples under `dotnet/` need the integrated host ABI and are
not wired into the S7 conformance ABI.

For an explicitly launched validation child, `--crash-probe native|panic|managed`
forces capture. Keep those artifacts outside `build/` and retain the original
report and symbols when testing. The existing S7 fixture remains a conformance
driver; it does not prove that Outpost gameplay or the complete engine ran.

## V2 verification

[distribution-v2.json](validation/distribution-v2.json) records real exports of the
Fox demo's five packed files through the release conformance template. Windows ran
120 headless ticks over 100,000 S7 test entities, with equal Rust/C# checksums. The
self-extracting installer was executed into a new directory and its files checked.
Windows ZIP/installer are about 1.7/2.1 MB; Linux tar.gz/AppImage about 2.7/3.6 MB;
Android APK/AAB about 11.5/4.0 MB. The refreshed Windows installer export took
14.6 s, Linux tar/AppImage 10.8/10.6 s and Android 34.6 s on the shared machine.
The earlier cold Windows portable export took 79.2 s; compiler time dominates.
Native crashes, Rust panic snapshots and actual NativeAOT C# unhandled exceptions
were captured from the exported Windows executable after loading C#. Their local
symbolized stacks and ARA2701 diagnostics are in the evidence. A separate child
test checks that a caught panic leaves capture working for a subsequent native
fault. Windows packages are unsigned; Android used a disposable upload test key.
Linux checks are static ELF/glibc 2.31 and embedded SquashFS hash verification;
no Linux launch, FUSE mount, Android device, real signing identity or store upload
is claimed.

To repeat the distribution test, stage the demo and conformance C# files as above
under `EVIDENCE/project/content` and `EVIDENCE/project/game`, and save prepared base
presets at `EVIDENCE/presets/{windows,linux,android}.export.json`. The Windows preset
needs `installerStub`, Linux needs `appimageRuntime`, and Android needs its explicit
test-key signing configuration. Configure the SDK environment shown below and the
password environments named in that preset. Then run:

```powershell
$env:CARGO_BUILD_JOBS = '2'
$env:ARA_DISTRIBUTION_EVIDENCE = '<new evidence directory>'
cargo test -p ara-export --test distribution_e2e -- --ignored --nocapture
```

`ARA_DISTRIBUTION_RESUME=1` reuses complete exports after checking every manifest
file and package hash; use it only while their inputs remain unchanged. Crash test
directories must be new, so stale dumps cannot satisfy a later failed capture.
Generated pins, binaries, private symbols and keys remain outside git; only the
canonical validation report is checked in.

## Integration entry points

`ara-export` owns orchestration; the protocol/CLI track can call
`export(&ExportRequest, &mut dyn EventSink)` and `doctor(&DoctorOptions, &Toolchain)`.
The temporary binary exposes `doctor`, `export`, `pin`, `artifact-pin`, `preset`,
`preset-fmt`, `schema` and `crash symbolize`.
It does not change the `ara` CLI. Dependencies are declared locally, without root
workspace edits. `ara-export-player` is an export **conformance fixture**, adapted
from Ara's S7 sample, until the merged engine supplies full `ara-player` templates.
Its sample component columns live in `bevy_ecs`; C# has no authoritative state.
Its ABI is the S7 test ABI, not a second production host ABI: real engine templates
must use the generated ara-host ABI. The fixture loads raw S7 media for validation;
production content should already come from the asset worker's platform cooker.
`doctor --project ROOT --preset PRESET` also checks the preset's target API and
configured keystore alias/password environment with a read-only keytool probe.

## Prepare and export the sample on Windows

```powershell
$env:CARGO_BUILD_JOBS = '2'
$env:ANDROID_HOME = 'D:/Android/sdk'
$env:JAVA_HOME = 'D:/Android/jdk'
$env:ARA_ZIG = '<Zig 0.17.0>/zig.exe'
$env:ARA_BUNDLETOOL = '<tools>/bundletool-all-1.18.3.jar'
rustup target add x86_64-unknown-linux-gnu aarch64-linux-android x86_64-linux-android
./crates/ara-export/prepare-sample.ps1
$env:ARA_ZIGCC = (Resolve-Path target/release/ara-zigcc.exe).Path
target/release/ara-export.exe doctor --platform android
target/release/ara-export.exe export `
  --project crates/ara-export/fixtures/sample `
  --preset target/export-presets/release/windows.export.json `
  --output target/sample-windows
```

Use `linux.export.json` and `android.export.json` for the other lanes. Output must
be a new directory; exports never overwrite a prior build. Template preparation is
separate from exporting: a normal export compiles only C# and explicitly declared
native plugins. Preparation defaults to release; `-Configuration dev` or `profile`
builds the bridge feature into the template. Release templates omit that feature.
The script saves/restores process environment and never starts WSL or emulators.
Tools can be reused through environment paths; production code has no spike paths.

Runtime pins carry `(platform, arch, config, hash, source)` in the preset. `source`
is a local path or HTTPS URL. Every cache hit and download is rehashed; a bad pin
fails with `ARA-EXP-TPL-*`. The cache stores objects by full BLAKE3 hash. Do not
change pins to accept unexpected bytes. Generate pins with the `pin` command.

## Presets and lanes

Presets are strict canonical JSON with `$schema: ara://schema/export/1`.
`ExportPreset::register` registers all types with `ara-schema`; `to_text` uses
schema order/default omission and the shared canonical writer. Fields and enum
values are camelCase, as in the kernel. `preset-fmt` validates and canonicalizes.

Windows and Linux default to `nativeAot`. Set `lane: readyToRun` for self-contained,
untrimmed CoreCLR and IL mod support. Trimming is deliberately disabled in this
lane. All Linux AOT links go through `ara-zigcc`, which verifies Zig **0.17.0** and
hard-pins `x86_64-linux-gnu.2.31`; there is no baseline override. Linux's final
static verification checks each ELF image's architecture, LOAD segments and glibc
requirements against 2.31. It never claims the executable ran. Windows smoke
verification launches the headless template, checks its result and captures logs
and C# timing statistics. Full engine templates must accept `--headless` and emit
one `ARA_RESULT` JSON object with `verified: true` on successful smoke completion.

Android builds both arm64-v8a and x86_64 using NativeAOT bionic RIDs and the NDK.
`aapt2`, deterministic ZIP writing, `zipalign`, `apksigner`, bundletool and
`jarsigner` produce signed APK **and** AAB without Gradle. Both ELF LOAD alignment
and APK stored-library offsets are checked at 16 KiB. The pack is stored as an
uncompressed, aligned APK asset. AABs are checked with bundletool and the local
structure validator. Missing AAB tools fail the export rather than skipping it.
The default minimum/target APIs are 29/36; `targetApi` and `versionCode` are preset
fields. Target API policy must be reviewed when Play changes its requirements.
References: [Play target API](https://developer.android.com/google/play/requirements/target-sdk),
[16 KB pages](https://developer.android.com/guide/practices/page-sizes),
[zipalign](https://developer.android.com/tools/zipalign).

`permissionManifests` contains project-relative plugin manifest fragments with
`uses-permission` / `uses-permission-sdk-23`. Namespaces are resolved; equal
declarations deduplicate and differing attributes fail with `ARA-EXP-AND-022`.
Other plugin manifest mutations fail explicitly; this is a permission merger.
Dev/profile adds INTERNET and debuggable; release does neither automatically.

Android defaults to a locally generated debug keystore and emits a warning.
For release/upload signing, set:

```json
"signing": {
  "keystore": "keys/upload.jks",
  "alias": "upload",
  "storePasswordEnv": "ARA_UPLOAD_STORE_PASSWORD",
  "keyPasswordEnv": "ARA_UPLOAD_KEY_PASSWORD"
}
```

Passwords are resolved by the signing tools from the named environment variables,
not passed as literal arguments or written into logs/presets. Windows Authenticode
uses the optional signing configuration described above.

## Packs and delivery

`ara-pack` v2 uses a canonical manifest plus 4 KiB-aligned zstd-9 chunks, bounded at
16 MiB raw each. `CookKey` includes platform, architecture, configuration, cooker
version and settings. Paths/timestamps never affect bytes. Entries name complete
file hashes plus ordered fixed-size chunk hashes. Repeated blocks deduplicate.
`diff` returns base/target identities, the target manifest, added/removed IDs and
download size. `stored_chunk` supplies precisely the bytes to upload/download.
`Pack::open` validates canonical metadata, ranges, overlaps and stored hashes;
reads and `validate_all` check bounded decompression and plaintext identities.
`MappedPack` maps files without copying; its unsafe open requires immutable files
for the entire mapping lifetime. Publish a content-addressed file before mapping.

`encryptionKeyEnv` optionally selects a 64-hex-character key. Chunks use
XChaCha20-Poly1305 after compression, with domain-separated deterministic keyed
nonces and chunk identities as authenticated data. Keys are absent from manifests.
This is an extraction deterrent, **not DRM or protection from a client owner**:
the runtime necessarily gets the key. Desktop fixtures read the configured key
environment variable; Android's fixture accepts `ARA_PACK_KEY` supplied by its
host. Full Android runtimes must connect platform key provisioning to RunConfig;
the exporter does not embed a secret in the APK. Signed APK/AAB reproducibility
is not promised: keys/signatures can introduce metadata. Content packs and ZIP
inputs are deterministic. The exact compressor/decompressor versions are locked.

## Events, diagnostics and verification

Each NDJSON event has `version: 1`, a monotonically increasing `sequence`, a `stage`
and `event: progress|artifact|warning|error|finished`. Progress has status/timing;
artifacts have path, byte length and BLAKE3. Diagnostics carry stable `ARA-EXP-*`
codes, a message, structured context, and fix commands. Tool failures additionally
carry program, exit code/timeout and log path; command arguments/environment are
not logged. An error returns a nonzero CLI exit / library Err. `finished` points
at canonical `report.json`. Consumers must not infer success from artifacts alone.
Compiler processes run below normal priority on Windows with two Cargo jobs,
disabled .NET build servers, private intermediate/output paths and deadlines.

Every completed export ends with verify. Reports separate `passed` from
`runtime_executed`. Without an explicitly selected Android device, signatures,
bundle validity, ABI/ELF/ZIP alignment and payload structure are verified and the
report says `android_structure`. Use `--device SERIAL`; existing emulators also
require `--allow-emulator`. No emulator is auto-started. Device runs select the
launched process's logcat stream, capture a PNG/perf result and stop the app.
Desktop conformance templates are headless: screenshots/GPU timings are recorded
as unavailable with explicit integration diagnostics, not invented evidence.
Failed jobs keep `cache/jobs/<id>/report.json` and compiler logs where writable.
Reports record `signing`, including `android-debug` versus `android-upload`.

## Bridge and device integration

`ara-bridge::Server::start` binds loopback WebSocket/TCP and requires a token of at
least 32 bytes. `Publisher::try_publish` uses bounded batches and never waits for
network I/O. The server owns its bounded log ring and frame/GPU histograms.
`DeviceClient` exposes correlated operations: hello, incremental structured logs,
PNG screenshots, perf, input actions, semantic scene queries and capture triggers.
`Adb` provides list/logs/screenshot/pull and RAII TCP forwarding. It preserves an
existing adb server; one it starts is stopped on drop. Tokens never enter URLs.
Drop `Server` to stop/join its worker. The caller must implement nonblocking hooks.

Wire `RuntimeHooks::screenshot` / `capture` to the renderer at frame boundaries;
publish real frame/GPU samples through `Telemetry`. Wire `input` to a bounded
typed action queue consumed by ara-sim at a fixed tick, and `scene` to read-only
semantic queries. Defaults return `ARA-EXP-BRG-010` through `013`, preserving the
missing-integration distinction. The fixture uses NoHooks and publishes its smoke
result; the real engine supplies these hooks without any document side writes.
Desktop dev fixtures use `ARA_BRIDGE_TOKEN` and `ARA_BRIDGE_PORT` (default 43871).
Android dev fixtures optionally read app-private `ara-bridge.json` with token/port,
written through `adb shell run-as PACKAGE`. Forward using `Adb::forward`, then use
the same DeviceClient API. Never package credentials or enable this in release.

## Apple handoff

`Platform::Macos` / `Ios` and arm64 template tuples exist; iOS rejects ReadyToRun.
`apple::probe` calls `ssh -o BatchMode=yes -o ConnectTimeout=10 HOST
ara-export-agent --capabilities` and requires `ara.export.agent/1` plus `arch: arm64`.
`AgentRequest` carries protocol, preset, pack hash and source archive hash; no
Windows-local paths cross that seam. Configure `macAgent` in the preset.

The Mac session must implement the `ara-export-agent --stdio` build/sign/verify
transport: negotiate capabilities, exchange source/pack objects by hash, compile
C# AOT/native plugins with Xcode SDKs, resolve arm64 templates, produce app/IPA,
codesign/provision/notarize, run local/device verification, and return the same
event/report/artifact contracts. Until that exists, lanes fail with
`ARA-EXP-APL-001` (no destination) or `002` (Mac implementation pending). SSH
destination validation rejects shell syntax; probing never modifies the Mac.
There is deliberately no fake Apple artifact or successful placeholder report.

## Validation

```powershell
cargo test -p ara-pack -p ara-bridge -p ara-export -p ara-export-player --all-features
cargo clippy -p ara-pack -p ara-bridge -p ara-export -p ara-export-player --all-targets --all-features -- -D warnings
cargo fmt -p ara-pack -p ara-bridge -p ara-export -p ara-export-player --check
$env:ARA_EXPORT_E2E = '1'
$env:ARA_EXPORT_PRESETS = (Resolve-Path target/export-presets/release).Path
$env:ARA_EXPORT_ARTIFACTS = Join-Path (Get-Location) 'target/export-validation'
cargo test -p ara-export --test end_to_end -- --ignored --nocapture
cargo test -p ara-export-player --features dev-bridge --test bridge -- --ignored --nocapture
```

Use a new `ARA_EXPORT_ARTIFACTS` directory for each end-to-end run. This keeps
reviewable reports/events and avoids overwriting a previous exported game.

The checked-in [Windows](validation/windows.json), [Linux](validation/linux.json)
and [Android](validation/android.json) reports record the final real sample run.
Windows executed 120 ticks / 100k entities with C# and Rust checksum
`0x825f3722a803f55c`; Linux used static ELF checks; Android used signed package
validation, without a device or emulator. Desktop NativeAOT output is 2.93 MB
(Windows) / 3.31 MB (Linux); the two-ABI APK/AAB are 7.31 / 3.07 MB. Export time
varied with concurrent host load: 11–52 s Windows, 36–63 s Linux, 29–88 s Android,
excluding template preparation. ReadyToRun was also smoke-verified on Windows
(7.3 s / 81.2 MB) and statically verified on Linux. The configured-keystore path
was tested separately with a disposable key and environment-referenced passwords.
