# ara-crash

Local crash artifacts, isolated native dump monitor, and offline symbolization.
`desktop::start` shares LOCALAPPDATA/Ara/crashes (macOS: ~/Library/Application
Support/Ara/crashes) for editor/Hub and offers to open
new reports at the next interactive launch. Bounded/headless runs do not prompt.
The per-app seen marker records report names, sizes and modification times, before
the dialog is offered; its own LastWriteTime is irrelevant. Headless runs do not acknowledge.
`ara-crash archive <pdb> <store>` converts matching private release symbols offline.
Capture uses crash-handler + minidumper on Windows (MiniDumpWriteDump), Linux
(ptrace/Breakpad minidumps) and macOS (Mach exception ports and a SIGABRT handler; the
task and thread go to the monitor over a Mach port). No SDK download or build tool.
macOS panic snapshots dump through a second short-lived client named after the panic
record (a dump request ends its client), and the hook waits for the callback.
macOS symbols: build with line tables and `split-debuginfo = packed`, archive the
DWARF file inside the `.dSYM`; minidump-writer may leave the main executable unnamed,
so symbolization also matches an unnamed module by debug ID.

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
