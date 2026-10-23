# ara-export

Production export orchestration; `ara-export` is temporary CLI wiring.
`export(ExportRequest, EventSink)` is the integration entry; `doctor` shares
tool discovery. `ExportPreset` is schema registered and uses ara-json canonical
writing. Template files must match the hash and (platform, arch, config) pin.

Run `cargo test -p ara-export`, `cargo clippy -p ara-export --all-targets`.
`cargo run -p ara-export -- doctor --platform windows` checks the host.
Always use CARGO_BUILD_JOBS=2. Tool output stays in logs, NDJSON stays on stdout.
Commands run out of process, below normal priority on Windows, with timeouts.
Only C# and explicitly declared native plugins compile during an export.
No implicit emulator/WSL launches. Linux supports static ELF verification here.
Apple uses the documented versioned SSH agent seam; no fake successful builds.
See README.md for template bootstrapping, integration and Apple handoff.
