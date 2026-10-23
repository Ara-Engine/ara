# ara-dev

Local developer binary. No engine dependencies; uses git and cargo as subprocesses.

- `check`: merge-base diff, reverse workspace dependency closure, hygiene, fmt,
  clippy, tests, diagnostic coverage and unchanged-tree verification.
  `--plan` previews selection; default deadline 40 minutes includes cache waits.
- `merge`: clean-tree no-ff merge, Cargo.lock-only recovery, workspace check,
  affected gate (or `--full`), commit and optional push.
- `gpu`: machine-wide FIFO file-lock queue with OS crash recovery and timeout.
- `clean-worktrees`: inspect targets; deletion requires `--yes`, merged branch,
  stale files, clean checkout, no symlinks/reparse points and no active Cargo lock.
- `disk`: all mounted drive free-space report.

Run `cargo test -p ara-dev`, `cargo clippy -p ara-dev --all-targets -- -D warnings`
and `cargo fmt --all -- --check`. Set CARGO_BUILD_JOBS=2 on the shared PC.
Integration fixtures and logs belong under `target/`, never in tracked paths.
JSON mode writes exactly one report to stdout; command output goes to log files.
Known failures require an explicit JSON file and exact test/output matches; fail closed.
Keep safety predicates pure and test them. Never silently restore test mutations.
See `docs/dev/ara-dev.md` for usage and recovery details.
`run.ps1` keeps bootstrap compilation separate and final binaries worktree-local.
