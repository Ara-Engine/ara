# ara-export

Production export orchestration; `ara-export` is temporary CLI wiring.
`export(ExportRequest, EventSink)` is the integration entry; `doctor` shares
tool discovery. `ExportPreset` is schema registered and uses ara-json canonical
writing. Template files must match the hash and (platform, arch, config) pin.
Conformance pins remain executable smoke workloads. Runtime pins are ZIPs of the
real ara-player plus cooked resources: `ara-export runtime-template --player EXE
--engine ENGINE --output ZIP`, then the existing `pin` command with matching symbols.
The builder includes current shader manifests/cohorts, excluding obsolete cooks.
Runtime packs contain `.ara/scene.cbor` and `.ara/cooked/<digest>.cbor`; their asset
source reads compressed cooked outputs directly, without author cache paths.
`examples/runtime_game.rs` probes a real authored project and requires an executed
game plus renderer screenshot. Captures and evidence go under target/.

Run `cargo test -p ara-export`, `cargo clippy -p ara-export --all-targets`.
`cargo run -p ara-export -- doctor --platform windows` checks the host.
Always use CARGO_BUILD_JOBS=2. Tool output stays in logs, NDJSON stays on stdout.
Commands run out of process, below normal priority on Windows, with timeouts.
Only C# and explicitly declared native plugins compile during an export.
Runtime pins use `scripts::prepare`: project `ara.module.toml` gameplay modules,
dependency graph, SDK generators/analyzers, then one NativeAOT `ara_script_init`
image. `game/scripts.json` carries ABI/layout identity and the linked inventory.
`ARA_DOTNET_SDK` selects SDK sources; `ARA_DOTNET_ARTIFACTS` selects prebuilt tools.
`tests/shipping_scripts.rs` exports a behaviour, relocates the pack/image, deletes
source, executes ticks and restores ECS state; stale ABI/layout must fail `ARA0921`.
`tests/runtime_csharp.rs` runs the full export and relocated standalone executable,
asserting C# ECS fields through `--read` and a stale-metadata `ARA0921` failure.
Run explicitly with prebuilt `ARA_SHIPPING_E2E_PLAYER` and its matching
`ARA_SHIPPING_E2E_PDB` (README); it retains evidence under target/.
No implicit emulator/WSL launches. Linux supports static ELF verification here.
Apple uses the documented versioned SSH agent seam; no fake successful builds.
See README.md for template bootstrapping, integration and Apple handoff.

V2 outputs `build/`, `distribution.json`, packages and private `symbols/<build-id>`.
Never package symbols, verification logs or crash artifacts with the player.
Windows pins require a matching PDB pin; ELF pins must retain DWARF until export.
`ara-crash` converts symbols and strips deployed ELF files through diagnosed LLVM
objcopy. The current CLI is ara-export; the CLI owner wires `ara crash symbolize`
to ara_crash::symbols::symbolize and registers ara_crash::why::CrashExplainer.
Managed capture is injected at compile time via a project-local MSBuild import,
without modifying the game project or any host ABI. No upload runs automatically.
Delivery formats are generic. No store SDKs, service settings or upload layouts.
