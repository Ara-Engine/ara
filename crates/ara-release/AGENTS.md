# ara-release

Windows engine distribution tooling, separate from exported games. `ara-release`
packages release binaries and runtime resources, generates notes, and tests an
installed distribution. The setup stub both installers start with is the
`ara-setup` crate (window, silent CLI, same-version replace; see its AGENTS.md);
packaging appends the portable ZIP to `target/release/ara-setup.exe`.

`cargo run -p ara-release -- package --version 0.0.1-alpha`
`cargo run -p ara-release -- notes --version 0.0.1-alpha`
`cargo run -p ara-release -- smoke`

Always CARGO_BUILD_JOBS=4. Package is an explicit opt-in release build; no dev
server or full workspace build. Outputs, logs, smoke projects and symbols belong
under target/release-dist. Never tag, push, publish, or upload crashes unless explicitly requested.
Every test/capture/manual check sets ARA_CONFIG_DIR to an existing scratch dir
under target/. The harness fails without it. Smoke children override it with
their own scratch profile; failure cleanup also stops that profile's Hub.

`package --kind hub|engine` builds independent distributions. Hub installs into
LOCALAPPDATA/Programs/Ara Hub; its bootstrap service runs from Ara/Services/version/build.
`keygen --output <outside-repo-path>` prints only the public key; embed it in
ara-update/public-keys.txt before packaging. `sign` and `publish` read --key or
ARA_RELEASE_SIGNING_KEY (a path). Never store private keys/tokens in source.
`publish --repo Snowy7/ara --tag hub-v<version>|engine-v<version> --prerelease --dry-run`
creates a signed local folder. Without --dry-run, gh creates/uploads a draft then
publishes it; gh is a publishing prerequisite only. See docs/releases.md.

Version must match workspace.package.version; paths are never derived from an
unchecked version string. Repackaging uses a fresh stage and refuses to replace
an existing distribution. Remove only packager-owned target outputs explicitly
when re-running after a code change. Installer/uninstaller refuse unrelated or
existing directories, preserve user projects, and use Hub RPC for registration.

Notices use cargo metadata (including transitive packages) and actual license
files, plus checked-in vendor and shader-tool notices. Microsoft .NET is a
prerequisite; only open-source Roslyn/NuGet dependencies are redistributed.
Only `ara_player.pdb` also ships beside `ara-player.exe`: Windows runtime exports
pin that file and archive it privately. Other PDBs stay in the separate symbols ZIP
with binary hashes. The symbols command converts them through ara-crash when needed.

`ara-hub.exe` and its updater ship when `crates/ara-hub` exists; the engine installer
registers its build, installs/reuses the standalone Hub and points its Hub shortcut
there. No-system-integration smoke leaves shared installations alone; smoke
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

Gates: cargo check -p ara-release; cargo test -p ara-release;
cargo clippy -p ara-release --all-targets -- -D warnings; cargo fmt --all.
Smoke must clear VULKAN_SDK and use installed resources. Keep exact command logs
and failures; never turn a failed smoke step into a pass.
Smoke also installs the standalone Hub setup, seeds a distinct same-version service,
runs its windowless --service-check, lists engine templates and creates Starter.
Its LOCALAPPDATA/APPDATA and ARA_CONFIG_DIR all stay inside the smoke directory.
