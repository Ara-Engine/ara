# ara-import-worker

Disposable worker executable. Registers ara-importers and calls serve_worker.
Custom worker executables can register audio/animation/plugin importers too.
Stdout is reserved for the framed binary-safe worker protocol; log to stderr.

Run cargo test -p ara-import-worker (process integration tests), cargo check,
cargo clippy -p ara-import-worker --all-targets. Set CARGO_BUILD_JOBS=2.
Worker crash fixtures live only under tests; never add a crash switch to production.
Do not watch CI/PRs or spawn persistent background services here.

Process jobs honor ImportInput preset and thread_budget (default HQ / 16 threads).
HotReload orchestrates preview/HQ as separate disposable processes/cache actions.
Progressive integration tests gate HQ persistence to test boundary/failure races.
examples/progressive_bench measures real worker publication in release mode.
