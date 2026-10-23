# Ara export integration

`ara-export` owns orchestration; the protocol/CLI track can call
`export(&ExportRequest, &mut dyn EventSink)` and `doctor(&DoctorOptions, &Toolchain)`.
The temporary binary exposes `doctor`, `export`, `pin`, `preset-fmt` and `schema`.
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
is an external post-export operation; doctor reports the missing identity.

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
