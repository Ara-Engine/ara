# Export, packs and the device bridge (named codes, slots 8000–9999)

## ARA8001 export::SDK_001: .NET 10 SDK unavailable
owner: ara-export
shown as: ARA-EXP-SDK-001

Doctor could not confirm a .NET 10 SDK with `dotnet --version`. This required prerequisite check fails; C# publishing needs the SDK.

Causes:
- `dotnet` is missing from `PATH`, or the selected SDK has a major version other than 10.

Fixes:
- On Windows, run `winget install Microsoft.DotNet.SDK.10`; ensure `dotnet --version` selects 10.x.
- Run `ara-export doctor --platform <platform>` again from the export environment.

## ARA8002 export::SDK_002: Rust toolchain unavailable
owner: ara-export
shown as: ARA-EXP-SDK-002

Doctor did not find `1.99.1` in `rustc --version`. This is a warning for template and native plugin development; exporting prebuilt templates without native plugins does not compile Rust.

Causes:
- Rust is missing from `PATH`, or another toolchain is selected.

Fixes:
- Run `rustup toolchain install 1.99.1` and select that toolchain when developing templates or native plugins.

## ARA8003 export::SDK_003: Cargo unavailable
owner: ara-export
shown as: ARA-EXP-SDK-003

Doctor could not find `cargo` on `PATH`. This optional check warns about the native plugin lane; a preset with native plugins needs Cargo to compile them.

Causes:
- Rust tooling is not installed or its binary directory is absent from `PATH`.

Fixes:
- Run `rustup toolchain install 1.99.1` and add the Rust installation's Cargo binary directory to `PATH`.

## ARA8004 export::TOOL_001: External tool failed
owner: ara-export
shown as: ARA-EXP-TOOL-001

An export subprocess exited unsuccessfully. The current operation stops; the diagnostic records the program, exit code and output log.

Causes:
- A compiler, packager, signer or verification command reported an error.

Fixes:
- Open the diagnostic's `log` file and correct the tool's first actionable error, then retry the export to a new output directory.
- Run `ara-export doctor --platform <platform>` when the log identifies a missing prerequisite.

## ARA8005 export::TOOL_002: External tool timed out
owner: ara-export
shown as: ARA-EXP-TOOL-002

A subprocess exceeded its operation deadline and was killed; the operation fails. Tool probes allow 10 seconds, script and native compilation 300 seconds, and Android packaging commands 180 seconds.

Causes:
- The tool stalled, waited for unavailable resources or exceeded the allotted time on this machine.

Fixes:
- Inspect the diagnostic's `log` and `program`; resolve blocked downloads, tool configuration or resource contention before retrying.
- If a healthy operation consistently needs longer, adjust its specific `tools::run` timeout in the exporter.

## ARA8006 export::CLI_001: Export CLI command failed
owner: ara-export
shown as: ARA-EXP-CLI-001

The CLI's command handler returned an error outside the pipeline's own failure reporting. The command exits unsuccessfully and emits the underlying error chain in `message`.

Causes:
- A preset or template file cannot be read, a preset is invalid, or a doctor, pin, schema or formatting operation failed.

Fixes:
- Read `message` for the underlying error or more specific `ARA-EXP-*` code and correct the named input.
- Run `ara-export --help` or `ara-export <subcommand> --help` to check the supported arguments.

## ARA8007 export::CLI_002: Doctor platform conflicts with preset
owner: ara-export
shown as: ARA-EXP-CLI-002

Doctor received both a preset and a different `--platform`. It rejects the request instead of checking prerequisites for the wrong platform.

Causes:
- The preset's `platform` and the CLI `--platform` value differ.

Fixes:
- Omit `--platform` when using `ara-export doctor --preset <preset> --project <project>`, or set it to the preset's platform.

## ARA8008 export::PRE_001: Export schema identity missing
owner: ara-export
shown as: ARA-EXP-PRE-001

The preset's `$schema` is not `ara://schema/export/1`. Parsing stops before the preset can be used.

Causes:
- The schema identifier is absent, misspelled or from another document format/version.

Fixes:
- Use an export preset with `$schema` set to `ara://schema/export/1`; inspect `ara-export schema` for its fields.
- Validate and canonicalize the corrected file with `ara-export preset-fmt <preset>`.

## ARA8009 export::PRE_002: Mobile requires NativeAOT
owner: ara-export
shown as: ARA-EXP-PRE-002

An Android or iOS preset selected a script lane other than NativeAOT. Preset validation blocks export.

Causes:
- The mobile preset sets `lane` to `readyToRun`.

Fixes:
- Set `lane` to `nativeAot`, or omit it to use the default; ReadyToRun is a desktop lane.

## ARA8010 export::PRE_003: Invalid pack chunk size
owner: ara-export
shown as: ARA-EXP-PRE-003

The preset's `chunkSize` is outside the supported range of 1 through 16,777,216 bytes. Export stops at preset validation.

Causes:
- `chunkSize` is zero or greater than 16 MiB.

Fixes:
- Use `chunkSize: 1048576` for 1 MiB chunks, or another byte count in the allowed range.

## ARA8011 export::PRE_004: Invalid assembly stem
owner: ara-export
shown as: ARA-EXP-PRE-004

The C# `assembly` value cannot safely name the expected native image. Preset validation rejects empty names or characters other than ASCII letters, digits and underscores.

Causes:
- `assembly` contains a path, extension, punctuation or an empty string.

Fixes:
- Set `assembly` to the project's emitted assembly stem, such as `Game`, and make the C# assembly name match.

## ARA8012 export::PRE_005: Invalid plugin library stem
owner: ara-export
shown as: ARA-EXP-PRE-005

A native plugin's `library` is empty or contains characters other than ASCII letters, digits and underscores. Export cannot determine the expected plugin artifact.

Causes:
- The preset supplies a filename or path instead of a library stem.

Fixes:
- Set `plugins[].library` to the Cargo library stem, without `lib` prefix, extension or directory components.

## ARA8013 export::PRE_006: Project path escapes root
owner: ara-export
shown as: ARA-EXP-PRE-006

A project-relative path resolves through a symlink or junction outside the canonical project root. The exporter refuses to read that input.

Causes:
- A script project, content directory, plugin manifest, permission manifest or keystore path traverses a link to another directory tree.

Fixes:
- Place the referenced input inside the project root and update the preset to its project-relative path.

## ARA8014 export::TPL_001: Template tuple mismatch
owner: ara-export
shown as: ARA-EXP-TPL-001

A template pin has the wrong platform, configuration or architecture, or repeats an architecture. Preset validation rejects the template set.

Causes:
- A pin was copied from another preset, or more than one pin names the same architecture.

Fixes:
- Match every pin's `platform` and `config` to the preset and include each supported architecture only once: `x64` for Windows/Linux, `arm64-v8a` and `x86_64` for Android, or `arm64` for Apple.

## ARA8015 export::TPL_002: Invalid template hash
owner: ara-export
shown as: ARA-EXP-TPL-002

A template pin's `hash` is not exactly 64 lowercase hexadecimal characters. The preset is rejected before template resolution.

Causes:
- The hash was truncated, uppercased or supplied in a different format.

Fixes:
- Generate the pin with `ara-export pin --platform <platform> --arch <arch> --config <config> --file <template>` and copy its BLAKE3 hash.

## ARA8016 export::TPL_003: Required template pin missing
owner: ara-export
shown as: ARA-EXP-TPL-003

A non-Apple preset does not have one runtime template pin for every required architecture. Export is blocked before compilation.

Causes:
- The Windows/Linux `x64` pin is missing, or one of Android's two ABI pins is absent.

Fixes:
- Add an `x64` pin for Windows/Linux, or both `arm64-v8a` and `x86_64` pins for Android; generate each with `ara-export pin`.

## ARA8017 export::TPL_004: Template hash mismatch
owner: ara-export
shown as: ARA-EXP-TPL-004

A cached, downloaded or local runtime template does not match its pinned BLAKE3 hash. Resolution stops; the exporter never silently substitutes a different pin.

Causes:
- The source artifact changed, a download is damaged, or the content-addressed cache entry is corrupt.

Fixes:
- Restore the artifact that matches the intended pin; replace a corrupt `<cache>/templates/<hash>` entry with that verified artifact.
- If deliberately upgrading the runtime, generate a new pin with `ara-export pin --platform <platform> --arch <arch> --config <config> --file <template>`.

## ARA8018 export::OUT_001: Output directory unavailable
owner: ara-export
shown as: ARA-EXP-OUT-001

The requested output already exists, or the exporter failed to create it when publishing. Existing exports are never overwritten; publication stops.

Causes:
- The destination was reused, another export claimed it, or filesystem permissions prevent creating it.

Fixes:
- Choose a new, writable `--output` directory and retry; give concurrent exports distinct destinations.

## ARA8019 pack::PACK_001: Invalid content pack
owner: ara-pack
shown as: ARA-EXP-PACK-001

Pack construction, reading or validation rejected malformed content, an unsupported format or a violated size/path/hash invariant. The pack operation fails; export cannot publish a validated pack.

Causes:
- A pack is truncated or corrupt, a manifest/chunk hash differs, or pack settings are unsupported.
- An input has a nonportable path, duplicate path, symlink, invalid JSON or excessive size.

Fixes:
- Use the detailed message to fix the named input; portable paths use `/` and exclude empty, `.` and `..` components, backslashes, colons and leading `@`.
- Regenerate a damaged pack from the original content with the matching Ara version; use the canonical writer for JSON inputs.

## ARA8020 pack::PACK_002: Pack encryption key unavailable
owner: ara-pack
shown as: ARA-EXP-PACK-002

Pack encryption or decryption lacks a usable matching key. The same code reports an unset key environment variable in the exporter or desktop player; encrypted content cannot be processed.

Causes:
- The preset's `encryptionKeyEnv` or the launch configuration's `encryption_key_env` names an unset variable.
- The key is not 64 hexadecimal characters or differs from the key used to encrypt the pack.

Fixes:
- Supply the same 32-byte key as 64 hexadecimal characters in the configured environment variable for export and desktop launch.
- The Android conformance player reads `ARA_PACK_KEY`; supply the matching key through its runtime environment.

## ARA8021 pack::PACK_003: Pack filesystem operation failed
owner: ara-pack
shown as: ARA-EXP-PACK-003

A filesystem operation used to build, open or map a content pack failed. The underlying I/O error explains why the pack is unavailable.

Causes:
- An input or pack file is missing, unreadable, locked or on a failing/full filesystem.

Fixes:
- Restore the named file or correct its permissions and available storage, then retry the pack operation.

## ARA8022 pack::PACK_004: Pack manifest conversion failed
owner: ara-pack
shown as: ARA-EXP-PACK-004

Serde could not serialize or deserialize a pack manifest value. The pack operation fails; malformed JSON caught earlier by the canonical parser may instead report `ARA-EXP-PACK-001`.

Causes:
- Manifest fields have incompatible types or required fields are missing, often after editing a pack or mixing format versions.

Fixes:
- Regenerate the pack with the matching Ara writer; inspect the serde message for the offending field instead of editing the binary manifest.

## ARA8023 export::VER_001: Runtime result missing
owner: ara-export
shown as: ARA-EXP-VER-001

Runtime output contains no `ARA_RESULT ` line. Verification cannot establish conformance and the export fails.

Causes:
- The runtime template does not emit the expected result, or startup stopped before completing the conformance run.

Fixes:
- Read `verify.log` and earlier launch errors; use a compatible runtime template that emits `ARA_RESULT` followed by JSON.

## ARA8024 export::VER_002: Runtime verification failed
owner: ara-export
shown as: ARA-EXP-VER-002

The runtime's `ARA_RESULT` JSON does not contain boolean `verified: true`. Export verification fails.

Causes:
- The conformance run reported a failed check, or the template emitted an incompatible result schema.

Fixes:
- Inspect the result and `verify.log` for the failing runtime check; correct the script/content/template mismatch and export again.

## ARA8025 export::VER_003: Script exception containment failed
owner: ara-export
shown as: ARA-EXP-VER-003

The runtime result includes `fault_contained` but it is not boolean `true`. Verification rejects the export because the script fault-containment check failed.

Causes:
- The script exception boundary failed its conformance probe or returned an incompatible result.

Fixes:
- Inspect `verify.log` and restore the script boundary's exception handling and error-status contract; rerun with a matching conformance template.

## ARA8026 export::VER_004: Device result timed out
owner: ara-export
shown as: ARA-EXP-VER-004

Android verification waited 20 seconds without finding `ARA_RESULT ` in logs filtered to the launched process. The export fails device verification.

Causes:
- The app stalled or terminated before logging its result, or the template does not emit the expected `ara` log message.

Fixes:
- Inspect `verify.log`, `launch.log` and `pid.log` for startup failures, then retry on the explicitly selected device.
- Ensure the Android template emits its `ARA_RESULT` line through the `ara` log tag.

## ARA8027 export::VER_005: Device runtime process missing
owner: ara-export
shown as: ARA-EXP-VER-005

After launching the Android activity, verification received an empty or nonnumeric process ID. It cannot select a live process for log collection.

Causes:
- The runtime exited during startup, or `pidof` returned an unexpected result such as multiple process IDs.

Fixes:
- Inspect `launch.log` and `pid.log`, then run `adb -s <serial> logcat -d -s ara:* AndroidRuntime:E` to diagnose startup.
- Check that the preset's `package` matches the installed application and launches one runtime process.

## ARA8028 export::VER_010: Verification capability unavailable
owner: ara-export
shown as: ARA-EXP-VER-010

Verification completed with a limitation that has no more specific named code. This is a warning, not failed verification; for Linux it currently records static ELF checks without a smoke launch.

Causes:
- The selected verification method did not execute the runtime or lacks a requested observation hook.

Fixes:
- Read `message` and the report's `verify.method` and `runtime_executed` fields to identify the missing evidence.
- For Linux runtime evidence, run the exported player on the intended Linux environment; the static verifier does not start WSL.

## ARA8029 export::WIN_001: MSVC locator missing
owner: ara-export
shown as: ARA-EXP-WIN-001

Doctor could not find `vswhere.exe` under `ProgramFiles(x86)/Microsoft Visual Studio/Installer`. This required Windows toolchain check fails.

Causes:
- Visual Studio Build Tools or its installer is absent from the expected installation path.

Fixes:
- Run `winget install Microsoft.VisualStudio.2022.BuildTools --override '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive'`, then rerun `ara-export doctor --platform windows`.

## ARA8030 export::WIN_002: MSVC toolchain not found
owner: ara-export
shown as: ARA-EXP-WIN-002

The `vswhere` probe did not return an installation with `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`. Doctor marks the Windows compiler/linker prerequisite as missing.

Causes:
- Build Tools is installed without the x64 C++ tools, or `vswhere` failed to query it.

Fixes:
- Run `winget install Microsoft.VisualStudio.2022.BuildTools --override '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive'`, then rerun `ara-export doctor --platform windows`.
- Ensure the installation includes the Windows SDK needed by the NativeAOT linker.

## ARA8031 export::WIN_003: Windows NativeAOT cache missing
owner: ara-export
shown as: ARA-EXP-WIN-003

Doctor did not find `runtime.win-x64.microsoft.dotnet.ilcompiler` in the NuGet package cache. This is a warning: publishing can restore the compiler package if package sources are reachable.

Causes:
- The Windows NativeAOT package has not been restored into `NUGET_PACKAGES` or the default user `.nuget/packages` directory.

Fixes:
- Run `dotnet restore <script-project> --runtime win-x64` with the project's NativeAOT settings and working package sources.
- Check `NUGET_PACKAGES` if the compiler is cached under another directory.

## ARA8032 export::WIN_004: Windows Rust target missing
owner: ara-export
shown as: ARA-EXP-WIN-004

Doctor did not find the Windows x64 MSVC Rust target. This warns about template/native plugin development; prebuilt-template exports without plugins do not need to compile Rust.

Causes:
- `x86_64-pc-windows-msvc` is absent from `rustup target list --installed`.

Fixes:
- Run `rustup target add x86_64-pc-windows-msvc` for the active toolchain.

## ARA8033 export::WIN_010: External Windows signing unconfigured
owner: ara-export
shown as: ARA-EXP-WIN-010

Doctor did not find `ARA_WIN_SIGN`. Authenticode is an optional external post-export step; setting this variable does not make the exporter sign binaries or validate a signing identity.

Causes:
- No external signing setup has been indicated through `ARA_WIN_SIGN`.

Fixes:
- When signing is required, configure the certificate and run `signtool sign /a /fd SHA256 dist/windows/ara-player.exe` against the actual exported executable.
- Set `ARA_WIN_SIGN` only as the doctor setup indicator; verify signing in your external signing process.

## ARA8034 export::WIN_100: Windows export stage failed
owner: ara-export
shown as: ARA-EXP-WIN-100

A Windows export stage failed without a more specific `ARA-EXP-*` code in its error chain. Export stops and preserves the job's failure report and logs when they can be written.

Causes:
- An uncoded filesystem, process-launch, serialization or pipeline error occurred in the reported stage.

Fixes:
- Read `message`, `context.stage`, `context.job` and the referenced logs in the error event or failure `report.json`; fix the underlying error.
- Run `ara-export doctor --platform windows` and retry with a new output directory.

## ARA8035 export::LNX_005: Linux Rust target missing
owner: ara-export
shown as: ARA-EXP-LNX-005

Doctor did not find the Linux x64 GNU Rust target. This is a warning for template/native plugin development, not a requirement for a prebuilt-template export without plugins.

Causes:
- `x86_64-unknown-linux-gnu` is absent from the active Rust toolchain.

Fixes:
- Run `rustup target add x86_64-unknown-linux-gnu`.

## ARA8036 export::LNX_006: Pinned Zig linker unavailable
owner: ara-export
shown as: ARA-EXP-LNX-006

The Linux linker lane requires Zig 0.17.0 for the fixed `x86_64-linux-gnu.2.31` target. Doctor or `ara-zigcc` could not locate or confirm that version; linking cannot proceed.

Causes:
- `ARA_ZIG` is missing or points to an invalid executable, or the detected Zig version differs from 0.17.0.

Fixes:
- Install Zig 0.17.0 and set `$env:ARA_ZIG='<path to zig 0.17.0>/zig.exe'` in PowerShell.
- Run `ara-export doctor --platform linux`; `ara-zigcc` itself requires `ARA_ZIG` even when doctor finds Zig on `PATH`.

## ARA8037 export::LNX_007: Linux linker wrapper missing
owner: ara-export
shown as: ARA-EXP-LNX-007

The production `ara-zigcc` executable was not found. Linux NativeAOT publishing and Linux native plugins need this wrapper to enforce the glibc baseline.

Causes:
- `ARA_ZIGCC` is unset or invalid and no `ara-zigcc` executable exists beside `ara-export`.

Fixes:
- Run `cargo build -p ara-export --bin ara-zigcc`, then set `ARA_ZIGCC` to that executable or place it beside `ara-export`.
- Run `ara-export doctor --platform linux` to confirm discovery.

## ARA8038 export::LNX_008: Linker option value missing
owner: ara-export
shown as: ARA-EXP-LNX-008

`ara-zigcc` encountered a trailing `-target`, `--target` or `-fuse-ld` with no following value. The wrapper rejects the malformed invocation before running Zig.

Causes:
- The compiler's argument list or expanded response file ends with an incomplete target/flavor option.

Fixes:
- Correct the invoking build configuration or response file to provide the option's value or omit the option; the wrapper enforces its own target and LLD flavor.

## ARA8039 export::LNX_010: glibc requirement exceeds baseline
owner: ara-export
shown as: ARA-EXP-LNX-010

An exported ELF image contains a numeric `GLIBC_*` requirement above 2.31. The static audit rejects the export because it exceeds the pinned Linux compatibility baseline.

Causes:
- A runtime template, script library, plugin or bundled runtime was linked against a newer host glibc.

Fixes:
- Rebuild the named native image with Zig 0.17.0 through `ara-zigcc`, targeting `x86_64-linux-gnu.2.31`; replace incompatible bundled binaries.
- For Rust Linux builds, set `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` to `ara-zigcc` and `ARA_ZIG` to the pinned Zig executable.

## ARA8040 export::LNX_011: Linux ReadyToRun cache missing
owner: ara-export
shown as: ARA-EXP-LNX-011

Doctor did not find `microsoft.netcore.app.runtime.linux-x64` in the NuGet cache. This optional warning indicates that the ReadyToRun lane may need to download its bundled runtime.

Causes:
- The Linux x64 runtime has not been restored into the selected package cache.

Fixes:
- Run `dotnet restore <script-project> --runtime linux-x64` with working package sources; check `NUGET_PACKAGES` if the cache lives elsewhere.

## ARA8041 export::LNX_012: Optional symbol tool missing
owner: ara-export
shown as: ARA-EXP-LNX-012

Doctor did not find the host `llvm-objcopy` executable in the detected Android NDK. This is a warning: Linux NativeAOT export sets `StripSymbols=false` and keeps symbols when it is unavailable.

Causes:
- The NDK is not detected or its LLVM tool directory lacks `llvm-objcopy`.

Fixes:
- If stripped output is needed, point `ANDROID_NDK_HOME` at an installed NDK, or set `ANDROID_HOME` to its parent SDK so the exporter discovers the NDK.
- Otherwise accept the warning and retained symbols.

## ARA8042 export::LNX_100: Linux export stage failed
owner: ara-export
shown as: ARA-EXP-LNX-100

A Linux export stage failed without a more specific `ARA-EXP-*` code in its error chain. Export stops and retains the job's failure report and logs when possible.

Causes:
- An uncoded filesystem, process-launch, serialization or pipeline error occurred in the reported stage.

Fixes:
- Read `message`, `context.stage` and `context.job` in the failure event/report and inspect the referenced logs.
- Run `ara-export doctor --platform linux`, correct the underlying error and retry with a new output directory.

## ARA8043 export::LNX_105: No exported ELF images
owner: ara-export
shown as: ARA-EXP-LNX-105

The Linux verifier found no file beginning with the ELF magic bytes anywhere in the assembled output. Static verification fails because no Linux executable or library was audited.

Causes:
- The selected runtime template is not a Linux binary, or assembly produced the wrong payload.

Fixes:
- Check the Linux `x64` runtime template and emitted script/plugin artifacts; pin the correct ELF template with `ara-export pin --platform linux --arch x64 --file <template>` and export again.

## ARA8044 export::ELF_001: Invalid ELF image
owner: ara-export
shown as: ARA-EXP-ELF-001

The shared Linux/Android auditor rejected an image that is not little-endian ELF64 or has invalid program headers/load segments. The export cannot pass binary validation.

Causes:
- The image is truncated, has the wrong format/class/endianness, has no load segments, or declares segment data beyond the file.

Fixes:
- Replace the named runtime/library with a complete little-endian ELF64 build for the destination and regenerate its template pin if applicable.

## ARA8045 export::ELF_002: ELF architecture mismatch
owner: ara-export
shown as: ARA-EXP-ELF-002

The ELF machine field differs from the architecture expected for its destination. Verification rejects the image.

Causes:
- An ARM64 library was placed in an x86_64 location, an x86_64 library was placed in `arm64-v8a`, or the Linux template is not x64.

Fixes:
- Supply ARM64 ELF machine 183 for Android `arm64-v8a` and x86_64 machine 62 for Android `x86_64` or Linux `x64`; correct the template pin or artifact placement.

## ARA8046 export::AND_001: Android SDK unavailable
owner: ara-export
shown as: ARA-EXP-AND-001

The exporter could not discover an Android SDK directory. This required prerequisite failure blocks Android packaging.

Causes:
- `ANDROID_HOME` and `ANDROID_SDK_ROOT` are unset or invalid, and the default `LOCALAPPDATA/Android/Sdk` location is unavailable.

Fixes:
- Set `ANDROID_HOME` to the installed SDK root, for example `$env:ANDROID_HOME='D:\Android\sdk'`, then run `ara-export doctor --platform android`.

## ARA8047 export::AND_002: Android NDK unavailable
owner: ara-export
shown as: ARA-EXP-AND-002

Doctor could not confirm NDK r28 or later with a host `clang` executable. The Android native toolchain prerequisite check fails.

Causes:
- The detected NDK has an older or unreadable `Pkg.Revision`, or its host LLVM toolchain is missing.

Fixes:
- Run `sdkmanager 'ndk;28.2.13676358'` and set `ANDROID_NDK_HOME` to that NDK if automatic SDK discovery selects the wrong installation.
- Run `ara-export doctor --platform android` to verify the detected revision and compiler.

## ARA8048 export::AND_003: Android resource tool missing
owner: ara-export
shown as: ARA-EXP-AND-003

Android build-tools or its `aapt2` executable is unavailable. Resource/manifest linking cannot run, so Android packaging is blocked.

Causes:
- No build-tools directory was found under the SDK, or the newest selected installation is incomplete.

Fixes:
- Run `sdkmanager 'build-tools;35.0.0'` against the SDK selected by `ANDROID_HOME` and verify `aapt2` in the detected build-tools directory.
- Run `ara-export doctor --platform android`; repair the selected installation if a newer incomplete directory is taking precedence.

## ARA8049 export::AND_004: Android target platform missing
owner: ara-export
shown as: ARA-EXP-AND-004

Doctor requires `platforms/android-<targetApi>/android.jar` and a target API of at least 36. A missing platform or lower configured API fails this required check.

Causes:
- The SDK platform for the preset's `targetApi` is not installed, or `targetApi` is below 36.

Fixes:
- Set the preset's `targetApi` to at least 36 and run `sdkmanager 'platforms;android-36'` for API 36, substituting the actual target API when higher.
- Run `ara-export doctor --preset <preset> --project <project>` to check the preset's target.

## ARA8050 export::AND_005: Java runtime unavailable
owner: ara-export
shown as: ARA-EXP-AND-005

The Android toolchain needs Java 17 or later to run the signing and bundle tools. Doctor could not verify that version, or packaging could not find `java`.

Causes:
- `JAVA_HOME/bin/java` and `PATH` do not provide a usable Java 17+ executable.

Fixes:
- On Windows, run `winget install Microsoft.OpenJDK.17`; set `JAVA_HOME` to the JDK root or make its `bin` directory available on `PATH`.

## ARA8051 export::AND_006: Bundletool unavailable
owner: ara-export
shown as: ARA-EXP-AND-006

The exporter did not find the bundletool JAR named by `ARA_BUNDLETOOL`. Both APK and AAB output are required, so Android packaging cannot proceed without it.

Causes:
- `ARA_BUNDLETOOL` is unset or does not name an existing JAR file.

Fixes:
- Install bundletool and set `$env:ARA_BUNDLETOOL='<path>/bundletool-all-1.18.3.jar'`, then run `ara-export doctor --platform android`.

## ARA8052 export::AND_007: Android signing setup incomplete
owner: ara-export
shown as: ARA-EXP-AND-007

Without `signing`, Android exports use a generated debug key and report a warning: the artifacts are unsuitable for store upload. With explicit signing, an unusable keystore/alias or missing password environment blocks signing or fails the preset doctor check.

Causes:
- No upload signing configuration was supplied.
- The project-relative keystore or alias is wrong, or the variables named by `storePasswordEnv` and `keyPasswordEnv` are unset or contain incorrect credentials.

Fixes:
- Configure `signing.keystore`, `alias`, `storePasswordEnv` and `keyPasswordEnv` in the preset; supply the passwords through those environment variables.
- Check the key with `keytool -list -keystore <project-upload.jks>` and run `ara-export doctor --preset <preset> --project <project>`.

## ARA8053 export::AND_008: Android Rust targets missing
owner: ara-export
shown as: ARA-EXP-AND-008

Doctor did not find both Android Rust targets. This is a warning for developing templates/native plugins; the prebuilt-template lane itself does not compile Rust.

Causes:
- The active toolchain lacks `aarch64-linux-android`, `x86_64-linux-android` or both.

Fixes:
- Run `rustup target add aarch64-linux-android x86_64-linux-android`.

## ARA8054 export::AND_009: Android device verification unavailable
owner: ara-export
shown as: ARA-EXP-AND-009

No authorized Android device is available or selected. Doctor and a static-only export report a warning; an export that explicitly selected a disconnected or unauthorized serial fails device verification.

Causes:
- `--device` was omitted, or `adb devices -l` does not show the selected serial in state `device`.

Fixes:
- Connect and authorize the device, check `adb devices -l`, and export with `--device <serial>`.
- For an already running emulator, also pass `--allow-emulator`; the exporter never starts one automatically.

## ARA8055 export::AND_010: Android NativeAOT cache incomplete
owner: ara-export
shown as: ARA-EXP-AND-010

Doctor did not find both bionic NativeAOT runtime pack directories in the NuGet cache. This is a warning; publishing may restore them when package sources are reachable.

Causes:
- The `linux-bionic-arm64` or `linux-bionic-x64` NativeAOT runtime pack has not been restored.

Fixes:
- Run `dotnet restore <script-project> --runtime linux-bionic-arm64 -p:PublishAotUsingRuntimePack=true`, then repeat with `--runtime linux-bionic-x64`.
- Check `NUGET_PACKAGES` if the packages were restored to a different cache.

## ARA8056 export::AND_011: Android device tool missing
owner: ara-export
shown as: ARA-EXP-AND-011

The exporter could not locate `adb` under the SDK's `platform-tools` or on `PATH`. Doctor warns because device verification is optional; an explicitly requested device run cannot proceed.

Causes:
- Android platform-tools is absent or the configured SDK/PATH points elsewhere.

Fixes:
- Run `sdkmanager platform-tools`, confirm `ANDROID_HOME`, and rerun `ara-export doctor --platform android --devices`.

## ARA8057 export::AND_013: Invalid Android API settings
owner: ara-export
shown as: ARA-EXP-AND-013

Android preset validation requires `minApi >= 29`, `targetApi >= 36`, `targetApi >= minApi` and `versionCode > 0`. Export is blocked if any condition fails.

Causes:
- An API level is too low, the target is below the minimum, or the version code is zero.

Fixes:
- Set `minApi` to at least 29, `targetApi` to at least 36 and no lower than `minApi`, and `versionCode` to a positive integer.
- Install the matching SDK platform and validate the preset with `ara-export preset-fmt <preset>`.

## ARA8058 export::AND_014: Invalid Android package ID
owner: ara-export
shown as: ARA-EXP-AND-014

The Android `package` must contain at least two dot-separated components. Each component must begin with an ASCII letter and contain only ASCII letters, digits or underscores; otherwise preset validation blocks export.

Causes:
- The application ID is a single word, has an empty component or contains invalid characters.

Fixes:
- Use a package ID such as `dev.ara.game` that satisfies the component rules.

## ARA8059 export::AND_015: Android signing alias empty
owner: ara-export
shown as: ARA-EXP-AND-015

The preset has a signing configuration with an empty alias. Validation rejects it before packaging.

Causes:
- `signing.alias` was omitted from the intended configuration or explicitly set to an empty string.

Fixes:
- Set `signing.alias` to the existing upload-key alias shown by `keytool -list -keystore <project-upload.jks>`.

## ARA8060 export::AND_016: Android alignment tool missing
owner: ara-export
shown as: ARA-EXP-AND-016

Doctor could not find `zipalign` in the selected build-tools directory. This required tool aligns the APK's native libraries and content for direct loading.

Causes:
- The selected SDK build-tools installation is missing or incomplete.

Fixes:
- Run `sdkmanager 'build-tools;35.0.0'`, repair the selected build-tools directory if needed, and rerun `ara-export doctor --platform android`.

## ARA8061 export::AND_017: APK signing tool missing
owner: ara-export
shown as: ARA-EXP-AND-017

Doctor could not find `lib/apksigner.jar` in the selected build-tools installation. APK signing and signature verification require this JAR.

Causes:
- The detected Android build-tools installation is incomplete.

Fixes:
- Run `sdkmanager 'build-tools;35.0.0'` and confirm `lib/apksigner.jar` exists in the build-tools directory selected by doctor.

## ARA8062 export::AND_018: JDK signing tools missing
owner: ara-export
shown as: ARA-EXP-AND-018

Doctor could not find both `keytool` and `jarsigner`. Android export needs them for keystore handling and AAB signing, so this prerequisite check fails.

Causes:
- Only a Java runtime is available, or `JAVA_HOME`/`PATH` points to an incomplete JDK.

Fixes:
- Run `winget install Microsoft.OpenJDK.17` on Windows and point `JAVA_HOME` at the full JDK root; rerun `ara-export doctor --platform android`.

## ARA8063 export::AND_020: Invalid plugin manifest
owner: ara-export
shown as: ARA-EXP-AND-020

A plugin permission manifest is malformed XML, lacks a `manifest` root, or contains a permission with no nonempty Android-namespaced `name`. Manifest merging stops.

Causes:
- A file in `permissionManifests` is malformed or declares `name` without the Android namespace.

Fixes:
- Fix the referenced XML to use a `manifest` root and `xmlns:android="http://schemas.android.com/apk/res/android"`; give every permission a nonempty `android:name`.

## ARA8064 export::AND_021: Unsupported plugin manifest content
owner: ara-export
shown as: ARA-EXP-AND-021

The plugin merger accepts only `uses-permission` and `uses-permission-sdk-23` elements, with Android-namespaced `name`, `maxSdkVersion` and `usesPermissionFlags` attributes. Unsupported content blocks packaging.

Causes:
- A plugin manifest includes activities, application nodes, other elements or unsupported/nonnamespaced permission attributes.

Fixes:
- Provide a permission-only manifest in `permissionManifests` using the supported elements and attributes.
- If a plugin requires other manifest features, implement that support in the merger before attempting to package it.

## ARA8065 export::AND_022: Conflicting plugin permission
owner: ara-export
shown as: ARA-EXP-AND-022

Two declarations with the same permission element type and name have different attributes. Merging fails instead of silently changing the permission's scope.

Causes:
- Plugins disagree on `maxSdkVersion` or `usesPermissionFlags` for the same permission.

Fixes:
- Make the declarations for the named permission identical after deciding the intended permission scope, or keep one authoritative declaration.

## ARA8066 export::AND_023: Invalid Android ZIP input
owner: ara-export
shown as: ARA-EXP-AND-023

The package ZIP reader rejected duplicate file names or a cumulative uncompressed size above 2 GiB. Packaging stops before accepting the archive.

Causes:
- A generated/imported ZIP contains repeated paths or expands beyond the reader's size limit.

Fixes:
- Regenerate the offending archive with unique file paths and remove unintended oversized inputs so its total uncompressed regular-file size is at most 2 GiB.

## ARA8067 export::AND_024: ELF load alignment invalid
owner: ara-export
shown as: ARA-EXP-AND-024

An ELF load segment has insufficient/non-power-of-two alignment or incompatible file and virtual-address offsets. Verification fails: Android requires 16,384-byte alignment; the shared Linux audit uses this same code with 4,096 bytes.

Causes:
- A native image was linked with incompatible page-size settings or has malformed load-segment metadata.

Fixes:
- For Android, rebuild the offending template or library with NDK r28+ and linker option `-Wl,-z,max-page-size=16384`.
- For Linux, rebuild the image with the pinned `ara-zigcc` lane and inspect the alignment value named by the diagnostic.

## ARA8068 export::AND_025: Android package manifest missing
owner: ara-export
shown as: ARA-EXP-AND-025

The final archive lacks its required manifest entry. APK/AAB structure verification fails.

Causes:
- An APK lacks `AndroidManifest.xml`, or an AAB lacks `base/manifest/AndroidManifest.xml` after packaging.

Fixes:
- Regenerate the package through the exporter's `aapt2` and bundletool pipeline; inspect `aapt2.log`, `aapt2-proto.log` and `bundletool-build.log` in the job directory.

## ARA8069 export::AND_026: APK native library alignment invalid
owner: ara-export
shown as: ARA-EXP-AND-026

A checked runtime/script library in the APK is compressed or its ZIP data offset is not a multiple of 16,384. Verification rejects it because direct native-library loading requires stored, aligned entries.

Causes:
- The APK was repacked with compression or modified after its alignment pass.

Fixes:
- Regenerate stored library entries, run `zipalign -P 16 -f 4 <unaligned.apk> <aligned.apk>` before signing, and verify the signed result with `zipalign -c -P 16 4 <signed.apk>`.

## ARA8070 export::AND_027: APK content pack alignment invalid
owner: ara-export
shown as: ARA-EXP-AND-027

The APK's `assets/game.arapack` entry is compressed or its data offset is not a multiple of four bytes. Verification rejects the package because the pack must support memory mapping.

Causes:
- A repackaging step compressed the pack or disturbed its ZIP alignment.

Fixes:
- Store `assets/game.arapack` without ZIP compression and regenerate alignment with `zipalign -P 16 -f 4 <unaligned.apk> <aligned.apk>` before signing.

## ARA8071 export::AND_028: App bundle configuration missing
owner: ara-export
shown as: ARA-EXP-AND-028

The AAB lacks `BundleConfig.pb`. Final structure verification fails.

Causes:
- An archive was assembled as a ZIP instead of a complete bundletool output, or repackaging removed the configuration.

Fixes:
- Regenerate the AAB through the exporter's bundletool `build-bundle` step and retain `BundleConfig.pb` when normalizing/signing the archive.

## ARA8072 export::AND_029: Emulator verification not enabled
owner: ara-export
shown as: ARA-EXP-AND-029

The selected serial begins with `emulator-`, but `--allow-emulator` was not supplied. Export stops before device installation/verification.

Causes:
- An emulator was selected without the explicit emulator opt-in.

Fixes:
- Repeat the export with `--device <emulator-serial> --allow-emulator` for an already running emulator, or select a physical device.

## ARA8073 export::AND_100: Android export stage failed
owner: ara-export
shown as: ARA-EXP-AND-100

An Android export stage failed without a more specific `ARA-EXP-*` code in its error chain. Export stops and preserves the private job's report and logs when possible.

Causes:
- An uncoded filesystem, process-launch, archive, serialization or pipeline error occurred in the reported stage.

Fixes:
- Read `message`, `context.stage` and `context.job`; inspect the referenced tool or packaging logs for the original error.
- Run `ara-export doctor --preset <preset> --project <project>` or `ara-export doctor --platform android`, correct the failure and retry with a new output directory.

## ARA8074 export::APL_001: Mac agent unconfigured
owner: ara-export
shown as: ARA-EXP-APL-001

An Apple export that this host cannot build itself (iOS, or macOS from a Windows or Linux host) has no `macAgent`. Apple builds require Xcode on an Apple silicon Mac; the exporter stops before compilation. A Mac exports macOS itself without an agent.

Causes:
- The iOS preset, or a macOS preset exported off a Mac, omits its Mac-agent SSH destination.

Fixes:
- Set `macAgent` to the intended Mac's SSH destination; this supplies configuration only, and the current build/sign/verify integration still reports `ARA-EXP-APL-002`.

## ARA8075 export::APL_002: Apple export integration unavailable
owner: ara-export
shown as: ARA-EXP-APL-002

The Mac-agent build/sign/verify path is an explicit implementation stub. Doctor reports a warning, and iOS exports, or macOS exports from a non-Mac host, fail even when `macAgent` is configured. Run macOS exports on an Apple silicon Mac instead.

Causes:
- The Mac session has not supplied and integrated `ara-export-agent` build, signing and verification support.

Fixes:
- Implement and integrate the Mac-agent workflow described in `crates/ara-export/README.md`; changing SDK paths alone cannot enable this lane.
- Use `ssh <mac-agent> ara-export-agent --capabilities` to check the agent contract while integrating it.

## ARA8076 export::APL_003: Invalid Mac SSH destination
owner: ara-export
shown as: ARA-EXP-APL-003

The capability probe rejected its SSH destination before launching `ssh`. Destinations may contain only ASCII letters, digits and `@._-`, and must not begin with `-`.

Causes:
- `macAgent` contains whitespace, shell syntax, a port suffix, URI syntax or a leading option marker.

Fixes:
- Use a destination such as `user@mac-host` or an SSH host alias; configure ports and other SSH options in the host configuration.

## ARA8077 export::APL_004: Mac agent capability mismatch
owner: ara-export
shown as: ARA-EXP-APL-004

The capability probe returned JSON whose `protocol` is not `ara.export.agent/1` or whose `arch` is not `arm64`. The remote agent is rejected.

Causes:
- The remote executable uses a different protocol version or reports an unsupported architecture.

Fixes:
- Install a matching `ara-export-agent` on an Apple silicon Mac and confirm `ssh <mac-agent> ara-export-agent --capabilities` reports `protocol: ara.export.agent/1` and `arch: arm64`.

## ARA8078 export::APL_005: SSH client unavailable
owner: ara-export
shown as: ARA-EXP-APL-005

Doctor could not find `ssh` on `PATH` for a macOS/iOS target. The Apple-agent transport prerequisite check fails.

Causes:
- The OpenSSH client is not installed or is absent from the export environment's `PATH`.

Fixes:
- On Windows, run `Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0` with the required system privileges, then run `ara-export doctor --platform macos` or `--platform ios`.

## ARA8120 export::APL_006: codesign unavailable or failed
owner: ara-export
shown as: ARA-EXP-APL-006

Signing or checking the macOS app bundle with `codesign` failed, or doctor could not find `codesign`. The export stops; nothing is published.

Causes:
- The Xcode command-line tools are not installed, so `codesign` is missing from `PATH`.
- A nested gameplay image or the bundle could not be signed, or a later check found the bundle's contents changed after signing.

Fixes:
- Run `xcode-select --install`, then `ara-export doctor --platform macos`.
- Read the `codesign-*.log` files in the job's `logs` folder; export again to a new directory rather than editing the bundle.

## ARA8121 export::APL_007: macOS app bundle is ad-hoc signed
owner: ara-export
shown as: ARA-EXP-APL-007

The macOS app is signed ad-hoc (`codesign --sign -`): it launches on the Mac that exported it, but Gatekeeper on other Macs rejects it. Developer ID signing and notarization are not configured yet. This is a warning; the export succeeds.

Causes:
- No Developer ID identity or notarization step is configured for macOS exports; ad-hoc signing is the default.

Fixes:
- Keep the build on the exporting Mac (local play and testing); Developer ID signing and notarization arrive as an explicit opt-in.

## ARA8122 export::APL_008: macOS runtime template incomplete
owner: ara-export
shown as: ARA-EXP-APL-008

A macOS runtime template or app bundle lacks what the Metal player needs: cooked Metal artifacts for a pipeline stage, or the game icon (`AppIcon.icns`, from the engine's `assets/brand/icons/game` PNGs).

Causes:
- A pipeline was cooked without Metal Shader Converter, so its stages have no `metal` artifacts.
- The template was built from an engine folder without the game icon PNGs, or one has the wrong size.
- The pinned template was not built from a macOS (Mach-O) player.

Fixes:
- Cook the engine's pipelines on the canonical cook host with Metal Shader Converter installed.
- Rebuild the template with `ara-export runtime-template --player <macOS ara-player> --engine <checkout> --output <zip>` and pin it again.

## ARA8123 export::APL_009: Native plugins unsupported on macOS
owner: ara-export
shown as: ARA-EXP-APL-009

The macOS export lane does not build native Rust plugins yet; the export stops before compiling anything.

Causes:
- The macOS preset lists `plugins`.

Fixes:
- Remove the plugins from the macOS preset, or export them for Windows, Linux or Android.

## ARA8079 export::APL_100: Apple export stage failed
owner: ara-export
shown as: ARA-EXP-APL-100

A macOS or iOS export stage failed without a more specific `ARA-EXP-*` code in its error chain. Export stops; the failure event identifies the actual platform and stage.

Causes:
- An uncoded filesystem, preset-processing or pipeline error occurred before a more specific Apple diagnostic was available.

Fixes:
- Read `message`, `context.platform`, `context.stage` and the job's failure report/logs.
- Run `ara-export doctor --platform macos` or `ara-export doctor --platform ios` for the actual target, correct the underlying error and retry.

## ARA8080 bridge::BRG_001: Bridge I/O failed
owner: ara-bridge
shown as: ARA-EXP-BRG-001

The device bridge encountered a socket, process or filesystem I/O error. The affected connection or device operation fails.

Causes:
- The runtime endpoint is unavailable, a port cannot be bound, an adb executable cannot be launched, or a transfer path is inaccessible.

Fixes:
- Read the underlying I/O message; check the dev/profile runtime's loopback address and port, adb path and any named local file.
- For Android transport problems, run `adb devices -l` and reconnect to the intended authorized serial.

## ARA8081 bridge::BRG_002: Bridge WebSocket failed
owner: ara-bridge
shown as: ARA-EXP-BRG-002

A WebSocket handshake or established connection failed or closed. The bridge operation cannot complete on that connection.

Causes:
- The client used raw TCP/HTTP instead of WebSocket, the peer disconnected, or a frame/message violated protocol or size limits.

Fixes:
- Connect to the loopback endpoint using `ws://<address>:<port>/` and the matching bridge client.
- Keep requests within the server's 1 MiB frame/message limits; reconnect if the runtime restarted.

## ARA8082 bridge::BRG_003: Invalid bridge JSON
owner: ara-bridge
shown as: ARA-EXP-BRG-003

JSON serialization or decoding failed in the bridge transport/client. The affected request, response or typed payload cannot be accepted.

Causes:
- The peer sent malformed JSON or fields incompatible with `Request`, `Response` or a typed result such as `PerfSummary`.

Fixes:
- Use protocol version 1 request/response types from `ara-bridge` and match the runtime/client versions; inspect the JSON error for the offending field.

## ARA8083 bridge::BRG_004: Invalid bridge server configuration
owner: ara-bridge
shown as: ARA-EXP-BRG-004

The dev bridge refuses to start unless its bind address is loopback and its token contains at least 32 bytes. No server is started.

Causes:
- The caller requested a non-loopback address or supplied a shorter token.

Fixes:
- Configure `Server::start` with `127.0.0.1` or `::1` and a private token of at least 32 bytes; use an adb forward for device access.

## ARA8084 bridge::BRG_005: Bridge protocol version mismatch
owner: ara-bridge
shown as: ARA-EXP-BRG-005

An authenticated request has a `version` other than 1. The server returns a diagnostic without executing the command.

Causes:
- The client uses a different wire protocol version.

Fixes:
- Send `version: 1` and use matching `ara-bridge` request types.

## ARA8085 bridge::BRG_006: Bridge authentication failed
owner: ara-bridge
shown as: ARA-EXP-BRG-006

The request token differs from the dev runtime's launch token. The command is rejected and the server closes the unauthenticated connection.

Causes:
- The client has a stale, missing or different launch token.

Fixes:
- Reconnect with the current dev runtime token in the JSON request's `token` field; keep it out of URLs and logs.

## ARA8086 bridge::BRG_010: Renderer screenshot hook unavailable
owner: ara-bridge
shown as: ARA-EXP-BRG-010

`RuntimeHooks::screenshot` has no renderer integration. Screenshot requests fail; headless desktop export verification also reports this as a warning while its conformance checks can still pass.

Causes:
- The runtime uses the default hook or a headless conformance template without renderer capture.

Fixes:
- Implement `RuntimeHooks::screenshot` using `ara-render` capture at a safe frame boundary and return PNG bytes.
- For a headless verification run, treat the report as runtime-conformance evidence without a rendered screenshot.

## ARA8087 bridge::BRG_011: Simulation input hook unavailable
owner: ara-bridge
shown as: ARA-EXP-BRG-011

The default `RuntimeHooks::input` cannot queue an input intent. The bridge request fails without changing simulation state.

Causes:
- The runtime has not installed the simulation input hook, or its simulation integration is unavailable.

Fixes:
- Implement `RuntimeHooks::input` through `ara-sim`, validating the declared action and applying the intent at the requested simulation tick.

## ARA8088 bridge::BRG_012: Semantic scene hook unavailable
owner: ara-bridge
shown as: ARA-EXP-BRG-012

The default `RuntimeHooks::scene` cannot answer semantic scene queries. The bridge returns a diagnostic instead of scene data.

Causes:
- The runtime has not installed a scene-query integration.

Fixes:
- Implement `RuntimeHooks::scene` through `ara-sim` with a validated, read-only semantic query.

## ARA8089 bridge::BRG_013: Capture trigger hook unavailable
owner: ara-bridge
shown as: ARA-EXP-BRG-013

The default `RuntimeHooks::capture` cannot schedule a renderer/profiler capture. The request fails without producing a capture.

Causes:
- The runtime has not installed a capture-trigger integration.

Fixes:
- Implement `RuntimeHooks::capture` through `ara-render`, validate the capture kind and schedule work at a safe frame boundary.

## ARA8090 bridge::BRG_014: Invalid bridge input action
owner: ara-bridge
shown as: ARA-EXP-BRG-014

The bridge rejected an input action before calling the simulation hook. Its name must be nonempty and at most 128 bytes; values must number at most four and be finite numbers in [-1, 1].

Causes:
- The action name is empty/too long, or the payload contains too many, nonfinite or out-of-range values.

Fixes:
- Send a declared action with a name of 1–128 bytes and at most four finite values in [-1, 1]; leave action-name validation to the installed simulation hook.

## ARA8091 export::BRG_015: GPU timing hook unavailable
owner: ara-export
shown as: ARA-EXP-BRG-015

Headless desktop export verification reports that GPU timing requires `ara-render`. This warning does not fail conformance verification; the report contains no measured GPU evidence.

Causes:
- The conformance template has no renderer GPU-timing integration.

Fixes:
- Integrate renderer GPU measurements and publish them in `ara_bridge::Telemetry.gpu_us` through the bridge publisher; update verification to report the measured evidence.

## ARA8092 bridge::BRG_020: Device bridge operation invalid
owner: ara-bridge
shown as: ARA-EXP-BRG-020

The device/client helper rejected an operation or response. This covers failed adb commands, invalid forwarding results or paths, non-loopback clients, mismatched response IDs/versions and missing or invalid screenshot data.

Causes:
- The selected device is offline/unauthorized, an adb operation failed, or a requested pull path is not absolute.
- The client connected to the wrong endpoint or received an incompatible response or non-PNG screenshot.

Fixes:
- Read the specific message; for adb failures run `adb devices -l` and use the intended authorized serial.
- Use an absolute device path for pulls and a loopback endpoint for clients; match protocol version 1 and request IDs, and return base64-encoded PNG bytes for screenshots.

## ARA8093 export::APPIMAGE_001: AppImage runtime pin missing or unavailable
owner: ara-export
shown as: ARA-EXP-APPIMAGE-001

The `linux-appimage` package requires a separately prepared, hash-pinned AppImage runtime. The preset or doctor check could not find the required pin or its local source.

Causes:
- `appimageRuntime` is absent from the preset, or its source is unavailable.

Fixes:
- Obtain an x86_64 type-2 AppImage runtime as an explicit setup step and run `ara-export artifact-pin --file <runtime>`.
- Put the resulting hash and source in `appimageRuntime`, then run doctor with the preset. The runtime is not a Cargo build prerequisite.

## ARA8094 export::APPIMAGE_002: AppImage runtime has the wrong format
owner: ara-export
shown as: ARA-EXP-APPIMAGE-002

The pinned runtime is not an x86_64 ELF image with the type-2 AppImage marker. Packaging stops before appending the SquashFS payload.

Causes:
- The downloaded runtime targets another architecture, uses another AppImage format, or is not a runtime executable.

Fixes:
- Replace it with an x86_64 type-2 runtime and regenerate its artifact pin.

## ARA8095 export::APPIMAGE_003: AppImage payload verification failed
owner: ara-export
shown as: ARA-EXP-APPIMAGE-003

Inspection of the embedded SquashFS found a payload hash mismatch, missing files or a nonexecutable `AppRun`. The exporter rejects the incomplete package.

Causes:
- A payload file was omitted or changed during packaging, or the launcher lost its executable mode.

Fixes:
- Re-export to a new output directory from the complete build folder and inspect the packaging error's file path.
- Ensure the AppImage writer includes every distribution file and marks `AppRun` executable before retrying.

## ARA8096 crash::CRASH_001: Invalid crash configuration
owner: ara-crash, ara-export-player
shown as: ARA-EXP-CRASH-001

Crash settings or the player's build-scoped crash configuration are invalid. Configuring an upload receiver requires HTTPS and nonempty consent text; player configuration must use `ara.crash.config/1` and a 64-digit hexadecimal build ID.

Causes:
- An upload endpoint is insecure, contains a line break or has no disclosure text.
- The deployed crash configuration has the wrong format or an invalid build ID.

Fixes:
- Omit `uploadEndpoint` to keep uploads disabled, or configure an HTTPS receiver and text explaining the proposed upload.
- Regenerate the build's crash configuration through export rather than editing its format or build identity by hand.

## ARA8097 crash::CRASH_002: Crash upload not authorized
owner: ara-crash
shown as: ARA-EXP-CRASH-002

An upload request was declined or uploads are disabled because no endpoint is configured. No network process is started by this rejection; local crash artifacts remain available.

Causes:
- The player declined consent for this upload attempt.
- The crash settings have no `uploadEndpoint`, which is the default.

Fixes:
- Respect the player's decision and retain the report locally.
- For an optional upload flow, configure the receiver and disclosure text, then request explicit consent for the report before calling upload.

## ARA8098 crash::CRASH_003: Crash upload unavailable or failed
owner: ara-crash, ara-export
shown as: ARA-EXP-CRASH-003

A consented upload has an invalid artifact or the HTTPS transfer failed. Doctor also uses this code when checking the optional `curl` tool for a configured endpoint; export-host availability does not establish availability on a player's machine.

Causes:
- The artifact is not a regular file or exceeds the 128 MiB upload limit.
- `curl` is unavailable, or the receiver, TLS connection or 30-second transfer failed.

Fixes:
- Select the complete local crash artifact within the upload limit and inspect the transfer error.
- Provide `curl` on hosts offering the optional upload action and check the HTTPS receiver; retry only with the player's consent.

## ARA8099 crash::CRASH_004: Native crash monitor unavailable
owner: ara-crash, ara-export
shown as: ARA-EXP-CRASH-004

Native capture could not install its handler or start the out-of-process monitor. Doctor also uses this code to describe Windows/Linux/macOS capture support and the disabled-by-default upload policy.

Causes:
- A crash handler is already installed in the process.
- The executable could not launch its monitor, or the monitor did not become ready within five seconds.
- The IPC socket path exceeds the 103-byte UTF-8 budget, or its private directory cannot be created or accessed. The diagnostic includes the byte count or the monitor's startup error.
- Native capture was requested on an unsupported platform.

Fixes:
- Install capture once and retain its guard for the player lifetime; release the previous guard before reinstalling.
- Keep monitor argument dispatch in the shipped executable and inspect process-launch permissions and the reported startup error.
- Leave the short IPC fallback enabled, or set `ARA_CRASH_IPC_DIR` to a writable short directory. Export verification preserves this diagnostic and fails if native capture is unavailable.
- Use the native capture implementation on Windows, Linux or macOS.

## ARA8100 crash::CRASH_005: Matching debug symbols unavailable
owner: ara-crash, ara-export
shown as: ARA-EXP-CRASH-005

Export could not archive or convert matching native debug information. Windows templates require a hash-pinned PDB whose GUID and age match the PE image; ELF images must retain DWARF until symbols have been archived.

Causes:
- A Windows template lacks its PDB pin, has no PDB identity, or was paired with a PDB from another build.
- The ELF template was built without DWARF or stripped before export.
- Debug-symbol conversion failed on the supplied image or symbol file.

Fixes:
- Pin the matching executable and PDB together with `ara-export pin --platform windows --arch x64 --file <player.exe> --symbols <player.pdb>`.
- Build ELF templates with `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` and `CARGO_PROFILE_RELEASE_STRIP=none`; let export archive symbols before stripping deployed images.
- Keep the original image and its symbols from the same build and inspect the converter's underlying error.

## ARA8101 crash::CRASH_006: Invalid crash artifact
owner: ara-crash
shown as: ARA-EXP-CRASH-006

The local symbolizer could not read the supplied minidump or rejected an unsupported report or malformed managed crash record. Symbolization cannot produce reliable evidence from an invalid artifact.

Causes:
- A minidump is truncated, corrupt or is not a minidump.
- A JSON report uses an unknown format, or a managed record has invalid magic, oversized fields or trailing bytes.

Fixes:
- Use the original complete `.dmp`, `.crash.json` or `.managed` file produced by capture, retaining any paired dump and sidecar together.
- Retry symbolization with the local symbol store from the same build; if capture was interrupted, reproduce the crash to obtain a complete artifact.

## ARA8102 export::CRASH_007: DWARF stripping tool unavailable
owner: ara-export
shown as: ARA-EXP-CRASH-007

Export needs LLVM objcopy to strip deployed ELF images after archiving their DWARF. Doctor warns when the tool cannot be found; this is an export setup step, not a Cargo prerequisite.

Causes:
- `ARA_OBJCOPY` is unset or points to an unavailable tool, and no LLVM or Android NDK objcopy was discovered.

Fixes:
- Set `ARA_OBJCOPY` to an installed `llvm-objcopy` executable, or make LLVM/NDK tools discoverable in the export environment.
- Run doctor for the Linux or Android preset again before retrying export.

## ARA8103 export::DIST_001: Package format conflicts with the preset
owner: ara-export
shown as: ARA-EXP-DIST-001

The selected package format does not match the target platform, or `android-play` lacks release configuration and explicit signing. Preset validation stops before packaging.

Causes:
- A Windows package targets Linux or Android, or a Linux package targets another platform.
- `android-play` targets another platform, uses a nonrelease configuration or has no signing configuration.

Fixes:
- Select `windows-portable` or `windows-installer` for Windows, `linux-appimage` or `linux-tar` for Linux, or a plain folder for the target platform.
- For `android-play`, select Android, release configuration and explicit Android release signing settings.

## ARA8104 export::DIST_002: Distribution contains unsupported file paths
owner: ara-export
shown as: ARA-EXP-DIST-002

The build folder contains a symlink, nonregular file or paths that collide when compared without case. The exporter rejects these inputs rather than producing an ambiguous distribution.

Causes:
- The source folder contains linked or special files.
- Two payload paths differ only by letter case.

Fixes:
- Copy regular files into the distribution and remove links or special files.
- Rename colliding files and update their references so every payload path is unique without regard to case.

## ARA8105 installer::INSTALL_001: Invalid installer image or payload
owner: ara-export-installer
shown as: ARA-EXP-INSTALL-001

The installer stub is not an unsigned Windows PE image, or the assembled installer has an invalid trailer, archive range or payload hash. Assembly or extraction stops before installation is published.

Causes:
- A different executable or an already signed stub was supplied for assembly.
- The installer was truncated or its appended archive or trailer was modified.

Fixes:
- Rebuild and pin an unsigned `ara-export-installer` stub, then regenerate the installer from a complete payload.
- Apply Authenticode signing only after the payload is appended; obtain a fresh complete package if an assembled installer is corrupt.

## ARA8106 installer::INSTALL_002: Unsafe installer archive entry
owner: ara-export-installer
shown as: ARA-EXP-INSTALL-002

The archive contains an unsafe path, symlink, directory entry or duplicate path, including case collisions. Extraction is rejected before publishing the destination.

Causes:
- An entry uses traversal, an absolute path, backslashes, an alternate stream, a reserved Windows name or a trailing dot or space.
- Two entries resolve to the same path, or an entry is a symlink or directory rather than a payload file.

Fixes:
- Repackage regular files with relative forward-slash paths that use portable Windows filenames.
- Remove duplicate or case-colliding entries and regenerate the installer.

## ARA8107 installer::INSTALL_003: Installer archive exceeds its expansion limits
owner: ara-export-installer
shown as: ARA-EXP-INSTALL-003

The archive exceeds the 64 GiB expanded-size or 100,000-entry limit, or an extracted file's size does not match its declaration. Installation fails without publishing the staged folder.

Causes:
- The package contains too many files or too much expanded data.
- An archive is malformed or its declared and actual file sizes differ.

Fixes:
- Reduce the package to fit the supported limits and regenerate it from the original build folder.
- Replace a malformed installer with a complete regenerated package.

## ARA8108 installer::INSTALL_004: Installation destination already exists
owner: ara-export-installer
shown as: ARA-EXP-INSTALL-004

The requested destination exists, including an empty directory, or appeared while extraction was staged. The installer only publishes a new directory and does not overwrite an existing installation.

Causes:
- A previous installation or another process already created the destination.

Fixes:
- Choose a new destination with `--destination <new-directory>`.
- Check for another installer using that path before retrying.

## ARA8109 export::INSTALL_005: Installer stub pin missing or unavailable
owner: ara-export
shown as: ARA-EXP-INSTALL-005

The `windows-installer` preset lacks a pinned installer stub, or doctor cannot find its source. Packaging needs a separately built generic stub.

Causes:
- `installerStub` is absent, or its pinned local artifact is missing.

Fixes:
- Build `ara-export-installer` for Windows and run `ara-export artifact-pin --file <ara-export-installer.exe>` on its unsigned executable.
- Add the resulting hash and source as `installerStub` in the preset, then run doctor again.

## ARA8110 export::SIGN_001: Invalid Windows signing configuration
owner: ara-export
shown as: ARA-EXP-SIGN-001

The preset's optional Windows signing settings are invalid. Authenticode configuration must target Windows, name a certificate environment variable and use HTTPS for an optional timestamp receiver.

Causes:
- `windowsSigning` is configured for another platform or has an empty `certificateEnv`.
- `timestampUrl` is not HTTPS or contains a line break.

Fixes:
- Configure signing only on a Windows preset, name the environment variable holding the certificate-store thumbprint and use an HTTPS timestamp URL if needed.
- Omit `windowsSigning` when producing unsigned packages.

## ARA8111 export::SIGN_002: Authenticode signing tool unavailable
owner: ara-export
shown as: ARA-EXP-SIGN-002

Doctor could not find the optional `signtool` executable, or a requested signing operation cannot proceed without it. Unsigned portable builds and installers remain supported.

Causes:
- Windows SDK signing tools are not installed or discoverable.
- `ARA_SIGNTOOL` points to an unavailable executable.

Fixes:
- Install the optional Windows SDK signing tools and set `ARA_SIGNTOOL` to `bin/<version>/x64/signtool.exe`, or expose the tool on `PATH`.
- Run doctor again; omit signing configuration if an unsigned package is intended.

## ARA8112 export::SIGN_003: Signing certificate reference unavailable
owner: ara-export
shown as: ARA-EXP-SIGN-003

The environment variable selected by `windowsSigning.certificateEnv` is unset or is not a 40-digit hexadecimal SHA1 certificate thumbprint. The certificate reference is checked without logging its value.

Causes:
- The export environment does not define the configured variable.
- Its value is a certificate path, another identifier or an invalid thumbprint.

Fixes:
- Set the named variable to the SHA1 thumbprint of the intended signing certificate in the Windows certificate store.
- Ensure the signing account can access that certificate and its private key, then retry signing.

## ARA8113 export::TPL_005: Player template contract mismatch
owner: ara-export, ara-export-player
shown as: ARA-EXP-TPL-005

The conformance executable received a runtime launch contract, or verification received a result that does not match the selected template contract. Conformance evidence cannot establish that an integrated runtime player executed the project.

Causes:
- The preset selects `playerTemplate: runtime` but pins a conformance executable.
- The player's result reports another contract or omits the contract identity.

Fixes:
- Use `playerTemplate: conformance` with S7 conformance template pins when validating packaging and crash reporting.
- For a runtime export, pin an integrated player implementing `ara.player.runtime/1` and require its headless result to match that contract and report verification.

## ARA8114 export::COOK_001: Runtime content cooking stalled
owner: ara-export, ara-play-host
shown as: ARA-EXP-COOK-001

No requested import completed within the export's stall timeout. The diagnostic lists pending assets and completed/total counts; prior completed generations remain cached. Total cooking time has no fixed limit while completions continue.

Causes:
- An importer is stalled, or every admitted import exceeds the no-completion interval.
- The shared machine is under sustained resource pressure.

Fixes:
- Inspect the listed source assets and import diagnostics, then retry; completed cache entries are reused.
- For exceptionally expensive cooks, set `ARA_EXPORT_COOK_STALL_SECONDS` to a larger positive integer (default 600).

## ARA8115 installer::INSTALL_006: Destination is not an Ara installation
owner: ara-setup
shown as: ARA-EXP-INSTALL-006

Setup was asked to replace a destination that is not a recognizable Ara installation of the payload's kind: an engine folder has `engine.json` with format `ara.engine/1`, a Hub folder has `hub.json` with format `ara.hub/1`. Setup never modifies such a folder.

Causes:
- `--destination` (or the default location) points at an unrelated folder, a link or junction, or the other kind of Ara installation.
- The installation's manifest is missing or damaged.

Fixes:
- Choose another `--destination`, or move the existing folder yourself after checking its contents.
- For a damaged Ara installation, delete or rename it, then run setup again.

## ARA8116 installer::INSTALL_007: Ara programs are running from the installation
owner: ara-setup
shown as: ARA-EXP-INSTALL-007

Replacing an installation needs every program started from that folder to exit (Ara Hub, its service, the editor, players, `ara` command lines and MCP servers). Silent setup reports them and changes nothing; the window lists them, offers Retry and stops the Hub service through `ara hub shutdown`. Setup never terminates processes.

Causes:
- Ara Hub, an editor, a player or an agent's `ara mcp` started from the installation is still open.
- `ara hub shutdown` failed, or the service did not exit within 15 seconds.

Fixes:
- Close the listed programs (agent sessions using Ara's MCP server included), then run setup again or select Retry.
- Stop the Hub service with `ara hub shutdown`; it starts again when it is needed.

## ARA8117 installer::INSTALL_008: Replacing the installation failed and was rolled back
owner: ara-setup
shown as: ARA-EXP-INSTALL-008

Setup extracts the new files beside the installation, moves the previous installation aside, then moves the new files into place. One of those moves failed; the previous installation is unchanged or was restored, and the extracted copy was removed.

Causes:
- A program, scanner or Explorer preview keeps a file inside the installation open.
- The new files could not be moved into place.

Fixes:
- Close programs that use files in the installation folder (C# compile servers included), then run setup again.
- If the problem persists, restart Windows and run setup before opening Ara.

## ARA8118 installer::INSTALL_009: Rollback failed during replacement
owner: ara-setup
shown as: ARA-EXP-INSTALL-009

The new files could not be moved into place and moving the previous installation back failed too. The previous installation is intact in a `.ara-replaced-*` folder beside the destination; the message names both paths.

Causes:
- Another process created or locked the destination while setup was replacing it.

Fixes:
- Rename the named `.ara-replaced-*` folder back to the destination, then run setup again.

## ARA8119 installer::INSTALL_010: Installation registration failed
owner: ara-setup
shown as: ARA-EXP-INSTALL-010

The installed package's `install.ps1 -RegisterOnly` (Hub engine registration, shortcuts, PATH and prerequisite checks) exited with an error. The installed files remain; the script's output follows the message and is copied by the window's Copy details.

Causes:
- A prerequisite such as the Microsoft Visual C++ v14 Redistributable (x64) is missing.
- The Hub service could not start or register the engine.

Fixes:
- Fix the cause shown in the output, then run `install.ps1 -RegisterOnly` in the installation folder (or run setup again and choose Replace).
- Check `hub-start.log` in the Ara config folder when the Hub service failed to start.
