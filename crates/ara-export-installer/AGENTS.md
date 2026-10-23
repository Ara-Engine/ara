# ara-export-installer

Prebuilt Windows self-extracting installer stub; no WiX, NSIS or runtime tool.
Exporter appends a ZIP and a length/hash trailer to this pinned executable.
`--destination DIR` installs into a new directory; `--list` only lists paths.
Installation stages into a unique sibling and renames only after validation.
It never overwrites or deletes an existing installation or creates services.
All archive names are validated before writing: no traversal, alternate streams,
Windows device names, symlinks or duplicate/case-colliding files. Failure removes
only the stage directory created by this invocation. Authenticode's PE certificate
table is honored when locating the trailer after signing.

Set CARGO_BUILD_JOBS=2. cargo test -p ara-export-installer;
cargo clippy -p ara-export-installer --all-targets -- -D warnings;
cargo fmt -p ara-export-installer -- --check.
Prepare once with cargo build -p ara-export-installer --release, then pin it.
The stub is tooling, never the game's runtime/player.
