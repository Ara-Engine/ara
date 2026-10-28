# ara-release

Windows engine distribution tooling, separate from exported games. `ara-release`
packages release binaries and runtime resources, copies reviewed notes, and tests an
installed distribution. The setup stub both installers start with is the
`ara-setup` crate (window, silent CLI, same-version replace; see its AGENTS.md);
packaging appends the portable ZIP to `target/release/ara-setup.exe`.

`cargo run -p ara-release -- package --version <workspace.package.version>`
`cargo run -p ara-release -- notes --version <workspace.package.version>`
`cargo run -p ara-release -- smoke`
`smoke --artifacts <existing release-dist>` tests packages without rebuilding them.
Engine packaging/`notes` copy root RELEASE_NOTES.md verbatim and require its version header.

Always CARGO_BUILD_JOBS=2. Package is an explicit opt-in release build; no dev
server or full workspace build. Outputs, logs, smoke projects and symbols belong
under target/release-dist. Never tag, push, publish, or upload crashes unless explicitly requested.
Every test/capture/manual check sets ARA_CONFIG_DIR to an existing scratch dir
under target/. The harness fails without it. Smoke children override it with
their own scratch profile; failure cleanup also stops that profile's Hub.

`package --kind hub|engine|all` builds independent distributions; all builds the
complete engine/Hub feature union once. CARGO_TARGET_DIR selects warm compiler
outputs; packages remain in this checkout's target/release-dist. Hub installs into
LOCALAPPDATA/Programs/Ara Hub; its bootstrap service runs from Ara/Services/version/build.
`keygen --output <outside-repo-path>` prints only the public key; embed it in
ara-update/public-keys.txt before packaging. `sign` and `publish` read --key or
ARA_RELEASE_SIGNING_KEY (a path). Never store private keys/tokens in source.
`publish --repo Ara-Engine/ara --tag hub-v<version>|engine-v<version> --prerelease --dry-run`
defaults to Ara-Engine/ara; `--repo` overrides ARA_RELEASE_REPO, then that default.
creates a signed local folder. Without --dry-run, gh creates/uploads a draft then
publishes it; gh is a publishing prerequisite only. See docs/releases.md.

Version must match workspace.package.version; paths are never derived from an
unchecked version string. Repackaging uses a fresh stage and refuses to replace
an existing distribution. Remove only packager-owned target outputs explicitly
when re-running after a code change. Installer/uninstaller refuse unrelated or
existing directories, preserve user projects, and use Hub RPC for registration.
Ship `paths.ps1` with both packages and any engine-installed Hub: their scripts
normalize verbatim drive/UNC prefixes before Windows PowerShell 5.1 provider calls.

Notices use cargo metadata (including transitive packages) and actual license
files, plus checked-in vendor and shader-tool notices. Microsoft .NET is a
prerequisite; only open-source Roslyn/NuGet dependencies are redistributed.
Only `ara_player.pdb` also ships beside `ara-player.exe`: Windows runtime exports
pin that file and archive it privately. Other PDBs stay in the separate symbols ZIP
with binary hashes. The symbols command converts them through ara-crash when needed.

`ara-hub.exe` and its updater ship when `crates/ara-hub` exists; the engine installer
registers its build, upgrades the standalone Hub by commit/generation and points all
Hub shortcuts there. `hub-installation.ps1` shares the updater/setup transaction lock,
rejects running/locked files, and rolls back failed directory swaps. No-system-integration
suppresses PATH/registry/Start-menu writes and registers the engine offline without
starting a Hub service; smoke also isolates LOCALAPPDATA so its
managed Hub never touches an existing user installation. Smoke
captures every Hub page from the installed binary. Packaged samples get their
`thumbnail.png` from `evidence/<main-scene>-player.png` (evidence is excluded).
Sample kits (`assets/kit/*`, git-ignored) are generated into the staged copy by
`ara-samples --samples <stage>/templates kits <name>`, so the checkout never drifts.
Then `references::check` gates every template: each `@path#id`, glTF URI, sidecar
source, look `scene` and project `main` must resolve inside it, or packaging fails
naming `document -> missing path` (`ara-release references <dir>` runs it alone).
Smoke opens every installed template in the installed editor (`--frames 20`).
It also creates Starter, queues `build.current` in an offscreen installed editor,
checks the resolved PDB hash, and requires verified `ARA_RESULT` after 8 game ticks.
The game launcher comes from distribution.json entryPoint. Smoke rejects unbranded launchers and private artifacts inside the upload folder; captures stay in report/.
That flow uses a human session to gracefully stop its owned editor authority.
Smoke invokes uninstall.cmd (the Apps entry point), requires exit 0, and checks
projects, settings, crash reports and user-added install files survive. The script
regression also requires exit 1 on refusal. Independent Hub services remain running.

Gates: cargo check -p ara-release; cargo test -p ara-release;
cargo clippy -p ara-release --all-targets -- -D warnings; cargo fmt --all.
Smoke must clear VULKAN_SDK and use installed resources. Keep exact command logs
and failures; never turn a failed smoke step into a pass.
Smoke also installs the standalone Hub setup, seeds a distinct same-version service,
runs its windowless --service-check, lists engine templates and creates Starter.
Its LOCALAPPDATA/APPDATA and ARA_CONFIG_DIR all stay inside the smoke directory.
Smoke seeds an older same-version Hub before engine setup, then launches both
portable shortcut aliases with windowless `--build-info` and checks the embedded commit.
Tiny SFX regression: set ARA_TEST_BIN_DIR to the built Hub/updater/setup binaries;
run `cargo test -p ara-release installed_packages_replace_a_stale_hub -- --ignored`.

`--features rehearsal` adds `rehearse --artifacts DIR --older-setup EXE --directory target/DIR`.
Package both kinds with that feature and an external throwaway signing key's public
key embedded at build time. It enables Hub's matching windowless rehearsal host and
the loopback-only `ARA_RELEASE_TEST_API` transport. Rehearsal markers block real publish.
The mock serves exact dry-run signed bytes and GitHub URLs, streaming large files.
It exercises the real Hub model, external updater/startup health, managed engine
install RPC, sample open and installed export's eight-tick ARA_RESULT. Every child
uses isolated config/local/roaming/crash roots, and cleanup stops owned processes.
