# Local developer infrastructure

Set `CARGO_BUILD_JOBS=2` on this shared Windows machine. GPU crates also need
`VULKAN_SDK` pointing to the installed SDK. All commands run from any directory in
the checkout; no Python, CMake, service or extra build tool is required.

## One gate

```
cargo run -p ara-dev -- check
cargo run -p ara-dev -- check --base main --json
```

The gate compares against the merge base of HEAD and the selected local ref. It
includes committed, staged, unstaged and untracked changes. Cargo metadata supplies
workspace paths and all dependency edges (including optional, build and test
dependencies). Changed packages and their reverse dependency closure run clippy
with all targets and warnings denied, then tests. Workspace manifests, Cargo.lock,
toolchain settings and shared build inputs invalidate the whole workspace;
documentation alone needs only hygiene and formatting. Deleted package paths
also invalidate everything rather than silently missing dependents.

Every run checks formatting, exact version pins in changed manifests, all tracked
and new AGENTS.md files (150 lines), built-in importer version literals in tests,
pending insta snapshots, and an unchanged working tree after Cargo finishes.
Content comparisons include files that were already dirty when the gate started.
Source/catalog changes also run ara-diag's diagnostic coverage test. A failed test
mutation is left visible for inspection; the gate never restores it silently.

Built-in ImportInput/ImportSettings fixtures and registry lookups should obtain
versions from the importer or installed registry metadata. Fake importer versions
used to test the asset cache contract remain valid fixtures.

Stdout contains the compact report or one JSON object. Cargo output and failure
details are retained under `target/ara-dev/run-*/`. Logs and generated test data
must stay under ignored target directories. `--full` runs workspace clippy/tests.

## One merge flow

```
cargo run -p ara-dev -- merge feature/branch
cargo run -p ara-dev -- merge feature/branch --push
cargo run -p ara-dev -- merge feature/branch --full
```

Start with a clean tree. The merge uses `--no-ff --no-commit`, saves the original
HEAD, and only repairs Cargo.lock conflicts automatically. It takes our lockfile
and lets Cargo's normal resolver retain existing versions while adding dependencies
required by the merged manifests. Other conflicts stop immediately. The workspace
must pass `cargo check --workspace --all-targets`, followed by the affected gate
against the saved HEAD, before a merge commit is created. Push is opt-in.

A failed gate leaves the merge uncommitted. Inspect the printed log, resolve the
problem, run `check --base <printed-original-HEAD>`, then commit manually. Use
`git merge --abort` to abandon the merge. The tool never resets or discards work.

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
