# Local developer infrastructure

Set `$env:CARGO_BUILD_JOBS='2'` in PowerShell on this shared Windows machine. GPU crates also need
`VULKAN_SDK` pointing to the installed SDK. All commands run from any directory in
the checkout; no Python, CMake, service or extra build tool is required.

On Windows, use `.\crates\ara-dev\run.ps1 check --no-compiler-cache --json` for the fast entry. It
builds the small tool through `%LOCALAPPDATA%\ara\dev-build`, keeping bootstrap
compilation out of engine Cargo locks. Final tool and engine executable names
stay under the worktree's `target/`; hashed engine intermediates retain the
configured `CARGO_BUILD_BUILD_DIR`, or the original Cargo target directory. The
script restores the caller's environment afterwards. The ordinary `cargo run`
entry remains supported.

`check --no-compiler-cache` (also supported by `merge`) clears `RUSTC_WRAPPER`
for child Cargo commands, preserving clippy's workspace wrapper. This uses
[Cargo's documented empty-wrapper override](https://doc.rust-lang.org/cargo/reference/environment-variables.html).
The Windows entry script also bypasses its bootstrap wrapper for this flag and
restores the caller's environment. It leaves the shared cache server and other builds running. On this contended
machine, a tiny optimized library probe took 20.70 seconds through sccache and
1.02 seconds with direct Rust. The flag is opt-in; ordinary commands retain the
caller's wrapper configuration. JSON reports include `compiler_cache_bypassed`
and total `elapsed_seconds`, including selection and hygiene, excluding bootstrap.

On 2026-10-24, a real entry-script change against `3cdff693` selected only ara-dev.
The warm gate passed in 70.69 seconds (77.97 seconds including bootstrap), with
formatting 10.70s, strict clippy 1.22s and tests 50.08s: 14 unit, nine CLI and two
GPU-queue tests passed. A deliberately invalid caller RUSTC_WRAPPER verified the
bootstrap bypass, and the script restored it afterwards. CARGO_BUILD_JOBS was 2.
The initial 33-package cold cache fill reached the 40-minute deadline while
compiling tests; it was reported as a timeout, with no test execution claimed.

Cargo 1.99 can reuse intermediate artifacts through `CARGO_BUILD_BUILD_DIR` while
keeping final executables in each worktree's local target directory. Prefer this
to sharing `CARGO_TARGET_DIR`: final executable names collide across branches.
Shared intermediate caches still serialize compilation and can rebuild when
branches use different linkers or feature sets. Cleanup inspects local target
directories; an external shared cache needs its own explicit cleanup policy.
Keep source changes and rebuilds out of an in-progress test/doctest run using that
cache: replacing its dependencies can invalidate already-built Rust metadata.

## One gate

```
cargo run -p ara-dev -- check
cargo run -p ara-dev -- check --base main --json
cargo run -p ara-dev -- check --plan --json
```

The gate compares against the merge base of HEAD and the selected local ref. It
includes committed, staged, unstaged and untracked changes. Cargo metadata supplies
workspace paths and all dependency edges (including optional, build and test
dependencies). Library/build changes and real dependency updates propagate through
reverse dependencies. Integration tests, examples and benches select their own
package; root documentation and AGENTS.md need hygiene and formatting. Crate
Markdown remains a build input because catalogs can use include_str!. Manifest comparisons resolve
workspace inheritance, features, defaults and locked versions. Moving an unchanged
dependency into workspace.dependencies, including a verified Rust import alias,
does not retest its consumers. Lockfile bookkeeping for a new workspace package
does not invalidate existing packages. Transitive version/source/checksum changes
select their consumers. A linker-only configuration change selects executable
packages to exercise final links and native interoperability; other shared build
settings and deleted package paths conservatively select the workspace.

Selected packages run clippy with all targets and warnings denied, then tests.
Cheap hygiene or compilation failures stop later build/test work. Tests continue
across failing targets so one run reports all failing suites. Diagnostic coverage
is reused when already passed in the selected test set. `--plan` prints the exact
selection and reasons without building; JSON plans say `planned`, never `passed`.

An affected gate has a total 2400-second deadline, including metadata and Cargo
cache waits. Timeout returns failure with the command log and terminates its
process tree (a kill-on-close Windows job). It cannot waive an unfinished test.
`--timeout <seconds>` overrides the budget; `--timeout 0` disables it. `--full`
selects workspace clippy/tests and has no default deadline for nightly use. The
deadline starts inside ara-dev; the Windows helper isolates the bootstrap build.

Every run checks formatting, exact version pins in changed manifests, all tracked
and new AGENTS.md files (150 lines), built-in importer version literals in tests,
pending insta snapshots, and an unchanged working tree after Cargo finishes.
Formatting covers every workspace package in batches of four, avoiding the
Windows command-line limit that cargo fmt --all exceeds with many test targets.
Content comparisons include files that were already dirty when the gate started.
Source/catalog changes also run ara-diag's diagnostic coverage test. A failed test
mutation is left visible for inspection; the gate never restores it silently.

Built-in ImportInput/ImportSettings fixtures and registry lookups should obtain
versions from the importer or installed registry metadata. Fake importer versions
used to test the asset cache contract remain valid fixtures.

Stdout contains the compact report or one JSON object. Cargo output and failure
details are retained under `target/ara-dev/run-*/`. Logs and generated test data
must stay under ignored target directories. `--full` runs workspace clippy/tests.
On Windows a gate that tests ara-dev moves its running image into a sibling
`ara-dev-runners` directory so Cargo can replace the original executable path.
The process and exit status stay attached to the caller; old runner images are
reclaimed on later runs.

Known main failures require explicit opt-in with
`check --known-failures docs/dev/known-main-failures.json` (also supported by
`merge`). The JSON array names each integration test's `package`, `target`, `test`,
required `output_contains` failure text and ownership `reason`. Accepted failures appear as `KNOWN` and in JSON's
`known_failures`; the gate can succeed with those exceptions. Every other failing
test, including a second test in the same target, still fails the gate. Compiler
errors, crashes, doctests, incomplete failure output and hygiene failures cannot
be waived. Entries that no longer fail are not reported; remove them once main
is fixed. Without the flag, all failures block the gate.

The current file records main's Valley duplicate-identity failure, owned by
runtime-polish. This infrastructure branch does not integrate its in-progress fix.

## One merge flow

```
cargo run -p ara-dev -- merge feature/branch
cargo run -p ara-dev -- merge feature/branch --push
cargo run -p ara-dev -- merge feature/branch --full
```

Start with a clean tree. The merge uses `--no-ff --no-commit`, saves the original
HEAD, and repairs Cargo.lock conflicts automatically, including alongside source
conflicts. It takes our lockfile
and lets Cargo's normal resolver retain existing versions while adding dependencies
required by the merged manifests. Other conflicts stop immediately. The workspace
must pass `cargo check --workspace --all-targets`, followed by the affected gate
against the saved HEAD, before a merge commit is created. Push is opt-in.

A failed gate leaves the merge uncommitted. Inspect the printed log, resolve the
problem, run `check --base <printed-original-HEAD>`, then commit manually. Use
`git merge --abort` to abandon the merge. The tool never resets or discards work.
After source conflicts, run `cargo check --workspace --all-targets` to regenerate
the lockfile, stage all resolutions including Cargo.lock, then run the gate.

## GPU timing queue

```
cargo run -p ara-dev -- gpu -- cargo run -p ara-render --example benchmark
cargo run -p ara-dev -- gpu --timeout 600 -- capture-tool --output target/capture.png
```

All worktrees use `%LOCALAPPDATA%\ara\gpu.lock` on Windows. Numbered FIFO tickets
live in `ara/gpu-queue`; a short queue lock serializes ticket allocation and
admission. Waiting commands print their position. The timeout is in seconds
(default 3600) and covers waiting, not command runtime. OS file locks release on
process exit; stale tickets are reclaimed without guessing whether a PID was
reused. Windows commands run in a kill-on-close job so wrapper termination also
terminates the protected command tree. Commands are executed directly with their
original arguments, without shell interpretation. Use an explicit shell if needed.

Normal GPU unit tests stay unwrapped. Benchmarks, timing runs and golden captures
must use the queue; it cannot coordinate commands that bypass the wrapper.

## Disk hygiene

```
cargo run -p ara-dev -- disk --warn-gib 20
cargo run -p ara-dev -- clean-worktrees
cargo run -p ara-dev -- clean-worktrees --dry-run --hours 12
cargo run -p ara-dev -- clean-worktrees --yes
```

Disk reports every mounted drive and returns failure below the warning threshold
(default 20 GiB). Cleanup lists registered worktrees and recursively measures each
local target directory. Inspection is the default. Deletion requires `--yes`, a
branch already merged into local main (`--base` selects another ref), no tracked
or untracked changes, and no file/directory writes within six hours (`--hours`).
Detached worktrees and the current worktree are always kept. Symlinks, junctions
and other reparse points are rejected. Existing Cargo profile locks must be idle
and are held through deletion. Paths, freshness, cleanliness and branch ancestry
are rechecked immediately before deleting exactly the canonical target directory.
No git worktree, branch, source file or shared target elsewhere is deleted.

## Shared dependencies and linker

Third-party packages used by at least three workspace crates inherit their
versions from root workspace.dependencies. Member declarations retain their
original feature sets and default-feature behavior. Compatible requirements use
the highest lower bound already present in the workspace; resolved versions in
Cargo.lock stay unchanged. Incompatible glam, libloading and tungstenite versions
have separate named workspace entries. Import aliases preserve existing Rust API
names, including ara-anim's public glam re-export.

All 37 third-party package families used by three or more crates are centralized.
The consolidation itself changes no Cargo.lock lines. Adding ara-dev introduces
only its package entry and feature edges for existing TOML packages;
it introduces no new third-party package versions. See `linking.md` for all three
heavy-target measurements and the Windows rust-lld configuration.

## Findings fixed while introducing the gate

Atlas and audio test fixtures now read installed importer versions; the sidecar
serialization contract uses a synthetic importer ID. This removes three literal
couplings without introducing a dependency cycle through ara-asset.

The networking process gate exposed a missed diagnostic perturbation when native
prediction starts after its requested tick. The hook now fires once on the first
available tick at or after that request. The process fixture requests tick one
before its baseline arrives, retaining the rollback assertion; the existing
listen/dedicated tests also cover a mid-session perturbation at tick 330. The
minimized process test failed before the fix (first prediction tick 20, no applied
perturbation, zero rollbacks) and passed after it. Temporary diagnostics were removed.

A 5 × 30-million-call Rust 1.99.1 opt-level-3 microbenchmark of the edited guard
measured medians 2.203 → 2.231 ns/call without an armed hook and 2.224 → 2.170 ns/call
with one. Sample ranges overlap on this shared machine. The normal movement
kernel is unchanged.

Serial test execution exposed a collaboration demo startup hang and a missing
physics probe hash: libtest prefixes child output with its test name. Both child
helpers now use quiet harness output so their protocols work with one test thread.
Main's `e8ffd90` gives runtime placeholders the distinct ARA7310 code and catalog
entry, preserving play-host's ARA7303 import failures. This branch follows that fix.

The compile-queue integration fixture uses explicit origins and stable source
files rather than depending on filesystem watcher timing relative to the agent
quiet window. It retains exact reload order, origin and changed-member assertions;
the compile server's queue unit test covers human preemption of pending agent work.
The reload fixture waits for its expected analyzer diagnostic as well as the error
report, so an earlier error notification cannot satisfy the next edit's assertion.

The Hub view-model lifecycle fixture now lives in ara's integration suite and uses
Cargo's CARGO_BIN_EXE_ara. Its former nested `cargo build -p ara` forced four jobs,
rebuilt another feature set, and guessed the CLI path from the intermediate test
directory. The CLI dev-dependency on ara-hub expresses the integration edge for
affected selection. All lifecycle assertions remain; the focused test passed in
5.41 seconds after building. Cargo.lock gains one existing workspace edge for it.
