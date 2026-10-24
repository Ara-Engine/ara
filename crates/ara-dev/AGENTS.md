# ara-dev

Local developer binary. No engine dependencies; uses git and cargo as subprocesses.

- `check`: integrator's batch gate: source owners/direct consumers get tests;
  all transitive consumers get clippy. Markdown never cascades; catalog edits run coverage.
  `--plan` previews selection; default deadline 40 minutes includes cache waits.
  `--no-compiler-cache` bypasses RUSTC_WRAPPER; JSON records total elapsed time.
  Compile tests and regular bins together for the full selection and test profile;
  record Cargo artifact paths for peer CARGO_BIN_EXE_* values without feature re-resolution.
  Native test workers share a RAM budget and resource groups; each suite has a deadline.
  `test-tiers.toml` labels slow GPU/.NET/process/capture suites; `--full` runs all.
- `cargo -- <args>`: focused checks with optional `ARA_SHARED_BUILD_DIR` reuse.
  Keep targets separate; the shared admission lease covers builds and test execution.
- `merge`: clean-tree no-ff merge, Cargo.lock-only recovery, workspace check,
  affected gate (or `--full`), commit and optional push.
- `gpu`: machine-wide FIFO file-lock queue with OS crash recovery and timeout.
- `clean-worktrees`: inspect targets; deletion requires `--yes`, merged branch,
  stale files, clean checkout, no symlinks/reparse points and no active Cargo lock.
- `disk`: all mounted drive free-space report.
  Also flags configured Cargo outputs inside another checkout's target.
- `perf`: optimized incremental build, once-frozen Outpost pack, GPU-locked frame
  windows, JSON/deltas, warm-player file controls and local stdio MCP bridge.

Agent checkpoints: `cargo test -p ara-dev`, `cargo clippy -p ara-dev --all-targets -- -D warnings`
and `cargo fmt --all -- --check`. Set CARGO_BUILD_JOBS=2 on the shared PC.
Integration fixtures and logs belong under `target/`, never in tracked paths.
Gate JSON mode writes one report to stdout; command output goes to log files.
`perf --keep-alive --json` streams one NDJSON report per completed window.
Known failures require an explicit JSON file and exact test/output matches; fail closed.
Keep safety predicates pure and test them. Never silently restore test mutations.
See `docs/dev/ara-dev.md` for usage and recovery details.
`run.ps1` keeps bootstrap compilation separate and final binaries worktree-local.
Cargo outputs default to this checkout's target. Explicit directories in another
checkout's target (including nested caches/junctions) are rejected before builds.
