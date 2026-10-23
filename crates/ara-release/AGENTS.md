# ara-release

Windows engine distribution tooling, separate from exported games. `ara-release`
packages release binaries and runtime resources, generates notes, and tests an
installed distribution. `ara-setup` reuses ara-export-installer's validated,
hash-checked SFX extraction and calls the per-user installer script.

`cargo run -p ara-release -- package --version 0.0.1-alpha`
`cargo run -p ara-release -- notes --version 0.0.1-alpha`
`cargo run -p ara-release -- smoke`

Always CARGO_BUILD_JOBS=2. Package is an explicit opt-in release build; no dev
server or full workspace build. Outputs, logs, smoke projects and symbols belong
under target/release-dist. Never tag, push, publish, or upload crashes.

Version must match workspace.package.version; paths are never derived from an
unchecked version string. Repackaging uses a fresh stage and refuses to replace
an existing distribution. Remove only packager-owned target outputs explicitly
when re-running after a code change. Installer/uninstaller refuse unrelated or
existing directories, preserve user projects, and use Hub RPC for registration.

Notices use cargo metadata (including transitive packages) and actual license
files, plus checked-in vendor and shader-tool notices. Microsoft .NET is a
prerequisite; only open-source Roslyn/NuGet dependencies are redistributed.
PDBs stay in a separate symbols ZIP with binary hashes; never inside the portable
archive. The symbols command converts these PDBs through ara-crash when needed.

Gates: cargo check -p ara-release; cargo test -p ara-release;
cargo clippy -p ara-release --all-targets -- -D warnings; cargo fmt --all.
Smoke must clear VULKAN_SDK and use installed resources. Keep exact command logs
and failures; never turn a failed smoke step into a pass.
