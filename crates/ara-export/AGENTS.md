# ara-export

Production export orchestration; `ara-export` is temporary CLI wiring.
`export(ExportRequest, EventSink)` is the integration entry; `doctor` shares
tool discovery. `ExportPreset` is schema registered and uses ara-json canonical
writing. Template files must match the hash and (platform, arch, config) pin;
`{from,to}_unpinned_text` keep runtime profiles pin-free for builders that pin at
build time (the editor's Build Current Profile); exports still validate pins.
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
image. `data/game/scripts.json` carries ABI/layout identity and the linked inventory.
`ara-export scripts --rid RID` (`scripts::build`) builds just that image and manifest,
for hosts without an export lane yet (osx-arm64 players; docs/dev/macos.md).
`ARA_DOTNET_SDK` selects SDK sources; `ARA_DOTNET_ARTIFACTS` selects prebuilt tools.
`tests/shipping_scripts.rs` exports a behaviour, relocates the pack/image, deletes
source, exercises all per-host callbacks in two worlds and restores ECS state; stale ABI/layout must fail `ARA0921`.
`tests/runtime_csharp.rs` runs the full export and relocated standalone executable,
asserting C# ECS fields through `--read`, continuous native UI, and a stale-metadata `ARA0921` failure.
Run explicitly with prebuilt `ARA_SHIPPING_E2E_PLAYER` and its matching
`ARA_SHIPPING_E2E_PDB` (README); it retains evidence under target/.
No implicit emulator/WSL launches. Linux supports static ELF verification here.
A Mac exports macOS itself: an ad-hoc signed `.app` (`src/macos.rs`; README "macOS
on a Mac"); iOS and other hosts use the documented SSH agent seam; no fake builds.
See README.md for template bootstrapping, integration and Apple handoff.

Exports output `build/<Game>.exe` (Linux: `<Game>`), one `build/data/` and notices;
macOS uses `<Game>.app` with `Contents/Resources` as its data root.
`distribution.json`, packages, `report/`, logs and `symbols/<build-id>` sit outside `build/`.
Never package symbols, verification logs or crash artifacts with the player.
Windows pins require a matching PDB pin; ELF pins must retain DWARF until export.
`ara-crash` converts symbols and strips deployed ELF files through diagnosed LLVM
objcopy. The current CLI is ara-export; the CLI owner wires `ara crash symbolize`
to ara_crash::symbols::symbolize and registers ara_crash::why::CrashExplainer.
Managed capture is injected at compile time via a project-local MSBuild import,
without modifying the game project or any host ABI. No upload runs automatically.
Preset `name` defaults to the project name; `version` stamps Windows resources and
macOS bundle metadata. `icon` is an authored-root-relative ICO/ICNS/SVG on Windows/macOS/Linux.
Without it players retain the engine's game icon. Branding precedes code signing.
Named presets resolve from `settings/builds/`; derived export data uses `.ara/export-cache`.
`preflight::runtime` checks installed companions, player/PDB GUID+age and complete
cooked manifests/runtime binaries and required Metal reflection before packaging;
offline Slang caches are excluded and existing shader hashes are checked.
`ARA-EXP-PREFLIGHT-001` offers `ara hub repair <version>` for an installed cohort;
projects using that version must close first. Pinned ZIPs get the same resource preflight.
Delivery formats are generic. No store SDKs, service settings or upload layouts.
