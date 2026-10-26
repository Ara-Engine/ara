# ara-crash

Local crash artifacts, isolated native dump monitor, and offline symbolization.
`desktop::start` shares LOCALAPPDATA/Ara/crashes for editor/Hub and offers to open
new reports at the next interactive launch. Bounded/headless runs do not prompt.
The per-app seen marker records report names, sizes and modification times, before
the dialog is offered; its own LastWriteTime is irrelevant. Headless runs do not acknowledge.
`ara-crash archive <pdb> <store>` converts matching private release symbols offline.
Windows/Linux capture uses crash-handler + minidumper (MiniDumpWriteDump on
Windows, ptrace/Breakpad minidumps on Linux). No SDK download or build tool.

`monitor_from_args` must run before any application initialization. `Capture`
owns the child process and native handler; drop detaches and reaps it. The crash
callback only requests a dump over preconnected IPC. Never allocate, symbolize
or upload from that callback. Rust's panic hook writes a local JSON report and
requests a paired native snapshot using ordinary messages, preserving the fatal
client connection for a later crash after a caught panic. Ping pins exception
context until the monitor finishes writing. Snapshot requests are thread-scoped.
Managed hosts report exception text/stacks through `write_report`; the export
fixture includes an AppDomain.UnhandledException hook without changing host ABI.

The `symbols` feature is tooling only: archive PDB/DWARF and Breakpad symbols
under one build ID and symbolize with rust-minidump. Players leave it disabled.
The `explain` feature supplies an ara-why Explainer for composition roots.
Uploads require configured HTTPS, consent text and explicit per-call consent.

Set CARGO_BUILD_JOBS=2. Run cargo test -p ara-crash --all-features,
cargo clippy -p ara-crash --all-targets --all-features -- -D warnings,
cargo fmt -p ara-crash -- --check. Process tests deliberately crash a child,
never the test runner. No crash artifacts enter a package or the repository.
