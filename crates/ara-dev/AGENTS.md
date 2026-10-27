# ara-dev

Local developer binary. Uses git/cargo subprocesses; ara-hotpatch is tooling-only here.

- `fmt` / `fmt --check`: workspace formatting in the gate's four-package batches;
  plain fmt writes; check leaves sources unchanged and fails on a formatting difference.
- `check`: integrator's batch gate: source owners/direct consumers get tests;
  all transitive consumers get clippy. Markdown never cascades; catalog edits run coverage.
  On main the default base is auto: checkout-local target/ara-dev/last-green.json.
  Missing/stale/foreign receipts require full bootstrap; explicit --base REF overrides.
  Only a clean, exception-free green run at unchanged HEAD advances the receipt.
  `--plan` previews selection; default deadline 40 minutes excludes package-cache waits.
  Doctests/coverage use that gate budget; `--timeout 0` leaves them unlimited.
  Gates default CARGO_INCREMENTAL=0 and optional installed sccache; explicit env wins.
  --no-compiler-cache bypasses it; shared intermediates retain Cargo's exclusive lane.
  Compile tests and regular bins together for the full selection and test profile;
  record Cargo artifact paths for peer CARGO_BIN_EXE_* values without feature re-resolution.
  Native workers adapt up to four within RAM/CPU limits; ARA_TEST_JOBS overrides.
  GPU/.NET/process resource groups stay exclusive; each suite has a deadline.
  `test-tiers.toml` labels slow GPU/.NET/process/capture suites; `--full` runs all.
- `cargo -- <args>`: focused checks with optional `ARA_SHARED_BUILD_DIR` reuse.
  Explicit CARGO_BUILD_JOBS is preserved; the unset default is two.
  Keep targets separate; the shared admission lease covers builds and test execution.
  Editor build/run pairs ara-player in one Cargo build, probes both RT values,
  and pins the exact companion artifact. Stale overrides/failed builds stop launch.
- `merge`: clean-tree no-ff merge, Cargo.lock-only recovery, workspace check,
  affected gate (or `--full`), commit and optional push.
- `gpu`: machine-wide FIFO file-lock queue with OS crash recovery and timeout.
  PDH guard: three-second baseline then 1 Hz foreign GPU sampling; owned descendants
  and their exited-counter tails are excluded. >10% summed over five samples or an
  actual game in gpu-processes.toml makes readings INVALID (exit 3), never FAIL.
  DWM counts above a 3% allowance; System PID 4/CSRSS stay informational.
  Vanished PDH instances retry; five seconds without valid data creates a bounded
  unmonitored gap. Only overlapping rows are invalid, including during reclassification.
  Idle services/launchers/overlays use measured GPU% only. --wait-for-quiet polls;
  no foreign process is altered. D26 invalidates overlapping rows only; saved
  readings can be re-scored with `d26 --reclassify <dir>` (see docs).
- `clean-worktrees`: inspect targets; deletion requires `--yes`, merged branch,
  stale files, clean checkout, no symlinks/reparse points and no active Cargo lock.
- `disk`: all mounted drive free-space report.
  Also flags configured Cargo outputs inside another checkout's target.
- `history-rewrite`: rehearse message cleanup in a fresh independent bare mirror;
  verify trees, metadata and ordered parents. Never rewrite a live checkout or push.
  `history-match` finds unique full-message/tree/metadata forks; see `docs/releases.md`.
- `perf`: optimized incremental build, once-frozen Outpost pack, GPU-locked frame
  windows, JSON/deltas, warm-player file controls and local stdio MCP bridge.
- `hotpatch`: `--install-dx` prepares optional pinned dx plus its native Rust save fix;
  ordinary builds never require it. Body-only source contract, guarded compiler/linker,
  frame-boundary player/editor publication. Windows facade stays inside this command.
  `render-probe` is a windowless pass proof; acquire the GPU queue before its start file.
- `preview`: isolate a workspace-version commit, package/smoke/sign both kinds
  locally. Only --publish pushes/uploads; --prepared resumes a reviewed record.
  --target-dir explicitly opts into warm external release outputs. See docs/releases.md.
- `d26`: the release scoreboard (`src/d26/`). It reuses `perf` windows on Outpost, Hangar and
  Showcase packs, `ara-samples capture --reference` (ꟻLIP) and the editor's responsiveness
  probe, and writes `target/d26/<ts>/report.{json,html}`. Unmeasured rows stay "not measured".
  GPU windows require raster/GI readiness then warmup; readiness timeouts fail loudly.
  CPU/hitches use whole-run samples; arrival peaks/time-to-ready have no budget gate.

Agent checkpoints: `cargo test -p ara-dev`, `cargo clippy -p ara-dev --all-targets -- -D warnings`
and `cargo run -p ara-dev -- fmt --check`. Set CARGO_BUILD_JOBS=2 on the shared PC.
Integration fixtures and logs belong under `target/`, never in tracked paths.
Gate JSON mode writes one report to stdout; command output goes to log files.
`perf --keep-alive --json` streams one NDJSON report per completed window.
Known failures require an explicit JSON file and exact test/output matches; fail closed.
Keep safety predicates pure and test them. Never silently restore test mutations.
See `docs/dev/ara-dev.md` for usage and recovery details.
`run.ps1` keeps bootstrap compilation separate and final binaries worktree-local.
Cargo outputs default to this checkout's target. Explicit directories in another
checkout's target (including nested caches/junctions) are rejected before builds.
