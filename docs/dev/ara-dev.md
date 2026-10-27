# Local developer infrastructure

`ara-dev preview` prepares signed local engine/Hub previews, including smoke tests;
only `--publish` uploads. See [the preview workflow](../releases.md#one-command-previews).

Set `$env:CARGO_BUILD_JOBS='2'` in PowerShell on this shared Windows machine. GPU crates also need
`VULKAN_SDK` pointing to the installed SDK. All commands run from any directory in
the checkout; no Python, CMake, service or extra build tool is required.

On Windows, use `.\crates\ara-dev\run.ps1 check --json` for the fast entry. It
builds the small tool through `%LOCALAPPDATA%\ara\dev-build`, keeping bootstrap
compilation out of engine Cargo locks. Final tool and engine executable names
default to the worktree's `target/`; explicit independent output directories remain
supported. Both target and build directories inside another checkout's target are
rejected, including nested caches and junctions. An unset build directory defaults
to this worktree's target, overriding global Cargo configuration. The script
restores the caller's environment afterwards. The ordinary `cargo run`
entry remains supported.

Workspace formatting: `cargo run -p ara-dev -- fmt` writes changes;
`cargo run -p ara-dev -- fmt --check` verifies without writing. On Windows,
`.\crates\ara-dev\run.ps1 fmt --check` is the bootstrap entry. Both use the gate's
four-package batches, keeping rustfmt below Windows' command-line limit.
Each batch has a log under `target/ara-dev/run-*`; failures return a nonzero exit.

On main, an omitted base means `--base auto`: only changes since this checkout's
last fully green gate are selected. Elsewhere the omitted base remains `main`.
Receipts live in `target/ara-dev/last-green.json`, independently of Cargo outputs.
Missing, rewritten, divergent or copied receipts require a full bootstrap, never
a zero-change pass. For the first deployment, use `--base <known-last-green>` on
the next batch; that green gate starts automatic tracking. Keep the receipt when
cleaning build caches. `--plan --json` previews the effective base and full scope.
See [the gate policy and measurements](gate-speed.md).

Gates default to `CARGO_INCREMENTAL=0`; an explicit environment value wins.
They use sccache when installed on PATH, preserve an explicit `RUSTC_WRAPPER`
(including empty to disable), and otherwise use normal Cargo compilation.
The shared-intermediate lane keeps its exclusive Cargo fingerprint cache instead.
`check --no-compiler-cache` (also supported by `merge`) clears `RUSTC_WRAPPER`
for child Cargo commands, preserving clippy's workspace wrapper. This uses
[Cargo's documented empty-wrapper override](https://doc.rust-lang.org/cargo/reference/environment-variables.html).
The Windows entry script also bypasses its bootstrap wrapper for this flag and
restores the caller's environment. It leaves the shared cache server and other builds running. On this contended
machine, a tiny optimized library probe took 20.70 seconds through sccache and
1.02 seconds with direct Rust. That historical probe explains the former bypass
recommendation; it does not measure today's cache. The bypass remains opt-in.
JSON reports include compilation policy, effective base, `compiler_cache_bypassed`
and total `elapsed_seconds`, including selection and hygiene, excluding bootstrap.

On 2026-10-24, an artifact-ambiguity regression against `cd7cebd8` selected only ara-dev.
The warm gate passed in 88.63 seconds (98.02 seconds including bootstrap), with
formatting 10.83s, strict clippy 1.72s, test compilation 4.50s and execution 59.48s:
16 unit, ten CLI and two GPU-queue tests passed. A deliberately invalid caller RUSTC_WRAPPER verified the
bootstrap bypass, and the script restored it afterwards. CARGO_BUILD_JOBS was 2.
The initial 33-package cold cache fill reached the 40-minute deadline while
compiling tests; it was reported as a timeout, with no test execution claimed.

Final validation on 2026-10-24 used source commit `3054c208`, after merging main
`2858d319`. The normal 40-minute affected gate selected 34 crates and passed in
848.96 seconds (851.27 including bootstrap): formatting 12.88s, diagnostic coverage
2.98s, strict clippy 16.35s, test compilation 265.15s, regular CLI binaries 39.33s,
and tests/doctests 498.14s. All 182 harnesses passed; the explicit Valley exception
file matched no failures. The working tree stayed unchanged and no pending
snapshots remained. The all-target workspace check also passed in 281.96 seconds.
Jobs were 2 and tests ran serially throughout.

After main integrated that implementation as `24a0ea8d`, this branch merged the
current tip as `5979db15`. The default gate passed again in 14.60 seconds, selecting
zero crates because only documentation remained different from main. The tracked
Cargo configuration retains rust-lld together with both relative scratch-profile
and crash-directory environment settings. This check did not rerun the 182
harnesses reported above.

A fresh renderer example build then captured every view mode through the real
machine GPU queue on the RTX 4070 SUPER, with validation clean. The 640x360 lit
image was visually inspected (material rows, glass, model, floor and shadows).
Local evidence is under `target/ara-dev/final-green3.json`, `final-workspace3.log`,
`final-capture3.log` and `verified-baseline/lit.png`. Disk reporting passed on all
drives, and the live worktree cleanup dry-run completed without deletions.

The gate compiles selected tests and regular binaries together with
`cargo build --lib --tests --bins --examples --profile test`, collecting Cargo's JSON
artifact messages in one invocation over the entire selected package set. This
keeps dev-dependency features unified, includes library-only feature owners, and
retains the test profile even when it differs from dev. It exposes the
exact regular executables as runtime CARGO_BIN_EXE_* variables for process-test
helpers. This keeps final binaries separate from hashed test executables, handles
custom build directories, and avoids a second build with a different feature set.
`test-build` accounts for all compilation, including peer binaries; `tests`
accounts for execution, including doctests. All phases share the same deadline.
Binary-only selections omit `--lib` and the doctest phase; library selections keep the
same package roots throughout binary preparation and test execution.

Cargo 1.99 can reuse intermediate artifacts through `CARGO_BUILD_BUILD_DIR` while
keeping final executables in each worktree's local target directory. Prefer this
to sharing `CARGO_TARGET_DIR`: final executable names collide across branches.
Shared intermediate caches still serialize compilation and can rebuild when
branches use different linkers or feature sets. Cleanup inspects local target
directories; an external shared cache needs its own explicit cleanup policy.
Keep independent build directories outside every checkout's target directory:
cleaning a parent target can remove a nested cache while its Cargo build is active.
Keep source changes and rebuilds out of an in-progress test/doctest run using that
cache: replacing its dependencies can invalidate already-built Rust metadata.

## Focused agents, one integration gate per batch

Build and run checkout editors through the paired development flow:

```powershell
.\crates\ara-dev\run.ps1 cargo -- build -p ara-editor --profile perf
.\crates\ara-dev\run.ps1 cargo -- run -p ara-editor --profile perf -- <project> --scene levels/outpost.scene
```

The wrapper includes `ara-player` in the same Cargo invocation, retaining every
selected package, feature, profile and target option. It takes both executable paths
from Cargo JSON, so custom target/build directories work. A run launches that editor
directly with the matching `ARA_PLAYER`, avoiding another feature resolution.
Explicit bin selection includes the player bin too. Workspace builds verify the pair
when they produce an editor; library-only builds and focused check/clippy/test commands
retain their normal scope. Native editor runs require explicit `-p ara-editor`.

Before editor warm-up, the companion must accept and advertise `--ray-tracing` with
both `false` and `true`; the probes only request help and use isolated scratch profiles.
Missing/incompatible companions, build failures, and an `ARA_PLAYER` override pointing
elsewhere stop launch with paths and a rebuild action. No flag is removed to accommodate
an old RT-default player. Logs are under `target/ara-dev/editor-*`. If an incompatible
executable was manually copied over Cargo's cached output, remove that stale final
executable and repeat the same paired command. Plain `cargo build -p ara-editor` and
manual executable launches cannot provide this companion check; use the wrapper.

Agents run fmt plus clippy/tests for touched crates, and the particular GPU/player/editor
suites their change touches. Commit and hand over the SHA. The integrator runs `ara-dev check`
once per merge batch on main; `--base` names the previous integrated checkpoint.
`--full` is the nightly/dependency-update validation path. Performance iterations use
`ara-dev perf` plus focused checks. Historical gate measurements above predate this policy.
Final full-gate scope, duration and curated manual hardware/export checks:
[`release-gate.md`](release-gate.md).

### Foreign GPU load

On Windows, `ara-dev gpu -- COMMAND` and D26 sample the native PDH
`\GPU Engine(*)\Utilization Percentage` counters. A three-second baseline
precedes the timed part; monitoring continues about once per second, including
a final sample. No GPU device or window is created by the guard. Own descendants
are excluded by PID/creation identity, including counter tails after child exit.
Counter buffers follow the Windows
[PDH wildcard-array API](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw).

Foreign utilization summed across engines/processes above **10% for five
consecutive samples** marks that time interval contaminated. DWM's first **3%**
is allowed for our own presentation; only its excess counts. System (PID 4) and
CSRSS are informational, because kernel paging and copy work can be ours.
Raw counters are retained, including full DWM usage, for policy reclassification.
Presence-only refusal is
restricted to actual game executables in `crates/ara-dev/gpu-processes.toml`.
Vanguard, Riot/Steam/Epic clients, Discord/NVIDIA overlays and other residents
do not block at idle: their measured GPU use counts normally. Unknown games
are caught by counters; launcher ancestry alone is not a reliable game identity.

Use `ara-dev gpu --wait-for-quiet -- COMMAND` or `ara-dev d26 --wait-for-quiet`
to wait for a quiet baseline instead of refusing. Ara never kills, suspends or
reconfigures foreign processes. Counter failures also invalidate timing evidence.
Transient NO_COUNTER, NO_INSTANCE and invalid-data reads refresh the wildcard
counter and retry at the next poll. A retiring individual instance is ignored
when the remaining instances are valid. A read failure never stops the sampler.
Only **five seconds without a valid sample** creates an unmonitored interval,
from the last valid sample through recovery; shorter churn stays informational.
The diagnostic is `ARA_GPU_FOREIGN_LOAD`; **exit code 3 means INVALID**, separate
from a child/test failure. A lease prints `ARA_DEV_GPU_READING {json}` on stderr
and writes its evidence path under `target/ara-dev/gpu-load/`. A contaminated lease
is INVALID. D26 retains values and invalidates only rows whose source window
overlaps contamination; clean rows keep their PASS/FAIL judgments. report.json
uses `partially_invalid` when only some measured rows overlap. GPU-load evidence
is in `gpu-load.json`: policy, separate intervals, names, PIDs, raw peak percentage
and UTC epoch-millisecond times. Separate bursts never bridge a quiet gap just
because the PID is the same. The first contamination is persisted
immediately; final reports retain the full sample history. A/B scripts must reject
exit 3 and rerun affected cases rather than report a speedup or budget failure.

Both commands accept `--foreign-gpu-percent 10`, `--foreign-gpu-samples 5` and
`--dwm-allowance-percent 3`. A high partial streak extends baseline sampling until
it resolves or reaches the sustained limit. An actual game still refuses immediately.

`ara-dev d26 --reclassify <reading-dir>` reapplies these defaults (or explicit
policy flags) to saved raw GPU samples and source metrics, without builds, counter
queries or launches. It preserves originals as `*.before-reclassify`, keeps the
engine source ID and numbers, and regenerates JSON/HTML judgments. Repeated
reclassification uses original counters, so the DWM allowance is subtracted once.
Monitoring retries and gaps are recorded separately from foreign load. Overlapping
rows have `invalid_reason: "unmonitored"`; earlier and recovered rows retain their
judgments. For old readings whose sampler died, the last valid sample and original
GPU evidence-file modification time bound the unknown tail. Preserve file times
when copying an old reading. Reclassification never invents clean samples after
the sampler stopped.

New runs record UTC child-call bounds in window sidecars. GPU rows use the steady
frame-duration envelope; CPU and hitch rows cover the whole run. Older player
reports have no absolute UTC fields: original launch-log creation and raw metrics
write times provide a labelled conservative envelope, with one PDH sample of
margin around steady frames. Editor scenarios without individual UTC markers use
their whole probe envelope. Resolved row windows are saved in report.json, so
later copies do not rely on changed filesystem dates. A gated source with missing
timing or incomplete monitoring coverage becomes **not measured**, never PASS.

The explicit `ARA_DEV_GPU_GUARD_FIXTURE` JSON stream is for subprocess tests only;
quiet fixture evidence is labelled `test_only`, never a native timing validation.
D26 refuses fixtures. Unit tests cover actual game presence, sustained aggregate
load, idle residents, owned descendants, exited tails, PID reuse and per-row status.
The recorded 20261026 desktop fixture keeps Outpost Low INVALID (DWM excess plus
desktop activity), while its Outpost Medium source window stays clean.

The normal gate tests source owners and **direct** consumers; clippy covers their full
reverse dependency closure. Test/example/bench edits stay local. Markdown (including
diagnostic catalog wording), docs and captures never cascade; fixture/golden changes
test their owner. Catalog edits always run `ara-diag --test coverage`. Rust token comparison
ignores formatting; Cargo.lock-only changes are reported without selecting source owners.
Use `--full` for dependency-only version upgrades. JSON/`--plan` distinguish `affected`
(compile) from `test_packages` (execution).

Tests execute compiled artifact paths directly, with one to four workers by default:
at most half the available CPUs and one worker per 512 MiB of the RAM budget.
Admission estimates reserve at most 4096 MiB and half the available physical RAM.
`ARA_TEST_JOBS` (1–4), `ARA_TEST_RAM_MIB`, and `ARA_TEST_TIMEOUT_SECONDS` override these limits.
The tracked `crates/ara-dev/test-tiers.toml` labels slow suites and exclusive GPU/.NET/process
groups. Fast suites have a 120-second deadline; slow suites have explicit 180–240-second
deadlines. The self-building `ara-dev / commands` suite has a 600-second Cargo-group
budget for contended fixture builds. The overall gate deadline still caps every suite;
`ARA_TEST_TIMEOUT_SECONDS` remains an explicit override. Timeout kills the process tree
and fails the gate. Separate suite logs and
`test-execution.json` record results/times. Doctests and diagnostic coverage remain checked
through Cargo using the overall gate deadline, without an independent phase cap.
`--timeout 0` leaves those Cargo phases unlimited. Reported Cargo package-cache
lock waits pause the execution budget until Cargo emits progress again.
No nextest installation is required; the built-in scheduler is used when that optional
developer tool is absent. This is an admission estimate, not an OS memory sandbox.

### Shared compilation on this PC

Cargo 1.99 shared intermediates reuse registry builds across worktrees. Keep final targets
separate and use the wrapper so builds **and tests** hold the common admission lock:

```powershell
$env:ARA_SHARED_BUILD_DIR='D:\AraBuild\ara-shared-dev'
$env:CARGO_BUILD_BUILD_DIR=$env:ARA_SHARED_BUILD_DIR
# Set CARGO_TARGET_DIR to YOUR worktree target, or leave it unset.
.\crates\ara-dev\run.ps1 cargo -- clippy -p ara-schema --all-targets -- -D warnings
.\crates\ara-dev\run.ps1 cargo -- test -p ara-schema
.\crates\ara-dev\run.ps1 check --base <last-batch> --json
```

This lane defaults to two Cargo jobs and honours an explicit `CARGO_BUILD_JOBS`
(the integrator can use four). It disables incremental output and bypasses the optional Rust
compiler wrapper. Cargo's shared fingerprints provide reuse without going through a second
compiler jobserver. It does not stop or reset another agent's sccache server. The existing
single disk cache is `D:\AraCache\sccache`; ordinary Cargo invocations keep their configured
wrapper. Unset the two shared-directory variables to return to independent local builds.
Never run an unwrapped Cargo build against this shared directory while tests use it.
Perf builds and merge pre-checks also take this lease. Perf releases it before entering
the continuous player/GPU loop, and keeps the requested profile's incremental setting.
Oversized suites run alone; RAM weights limit parallel admission rather than enforcing RSS.

Why not just set `SCCACHE_BASEDIRS`? The installed
[sccache 0.18 Rust implementation](https://github.com/mozilla/sccache/blob/v0.18.0/src/compiler/rust.rs)
hashes the working directory and Cargo environment; its base-directory rewriting covers C/C++.
Its [independent compiler jobserver](https://github.com/mozilla/sccache/blob/v0.18.0/src/jobserver.rs)
also cannot inherit each caller's Cargo budget. Cross-worktree workspace crates can still
recompile because their source/package identities differ; registry dependency reuse is
measured separately. We do not normalize embedded manifest paths or reuse branch binaries.

### Gate measurements, 2026-10-24

Measured on the shared 16-thread Windows PC with two build jobs:

- The real infra-only batch against `38fbcd49` passed with main's existing runner in
  **255.80s**, and the revised runner in **178.84s** (30.1% less wall time). Both select
  ara-dev; the revised run includes two additional regressions. Formatting fell from
  20.11s (101 crates) to 0.47s (one crate), test preparation 23.42s to 11.99s, and
  execution 192.33s to 155.87s. Revised execution passed 23 unit, 17 CLI and two queue
  tests, with a 2572 MiB admission budget. Strict clippy and unchanged-tree checks passed.
- A valid catalog sentence edit on isolated main `7bc3ad5e` selected **83 compile
  packages** with main's runner. The revised gate selected no compile/package-test
  roots, retained diagnostic coverage, and passed in **13.09s / 6.43s** (first/warm).
- A controlled four-crate dependency chain checked all four crates but tested two.
  Matched warm runs improved **24.15s / 21.42s → 14.59s / 13.56s**; harness execution
  improved **18.08s / 14.76s → 4.02s / 3.09s**. This separates selection/scheduling
  from the infra batch's intrinsically serial Cargo transaction fixtures.
- Three linked worktrees building the same three registry dependencies took
  **58.84s / 12.35s / 9.24s** with separate intermediates and the existing sccache,
  versus **3.04s / 0.568s / 0.572s** with shared intermediates. Both followers reused
  **3/3 registry artifacts**; workspace roots compiled on their first visit. The
  focused wrapper then reused all four artifacts with a deliberately missing optional
  compiler-cache executable configured (Cargo reported 0.41s).

The recorded main run `run-17344-1792844038180` reached **2402.21s** of observed wall
time with incomplete test output: clippy 4m26s, test build 12m05s, then slow player
failures/timeouts. It is an incomplete historical baseline, not a passing catalog-only
timing. The measurements above identify the selected workload and preserve that distinction.
Existing sccache counters showed 46.2% Rust hits and 19,401 incremental exclusions;
the new reuse percentages are Cargo artifact hits, a separate metric.

Evidence is retained under `target/ara-dev/infra-round/` (`infra-batch-before.json`,
`infra-batch-after-fixed.json`, `catalog-*.json`, `cache-results.json`, and focused logs).
The inherited fidelity shader cohort was re-cooked after the main merge: all 48 pipelines,
296.63s, with no tracked output changes. No GPU timing run was needed for this round.

## Performance iteration

Choose a target directory unique to this worktree, for example:

```powershell
$env:CARGO_BUILD_JOBS='4'
$env:CARGO_TARGET_DIR='D:\AraBuild\my-worktree-perf'
$env:CARGO_BUILD_BUILD_DIR=$env:CARGO_TARGET_DIR
cargo run -p ara-dev -- perf --preset high --frames 240
cargo run -p ara-dev -- perf --preset high --keep-alive
# From another shell, using the control directory printed by the first command:
cargo run -p ara-dev -- perf control --session <control-dir> --preset medium
cargo run -p ara-dev -- perf control --session <control-dir> --reload-shaders
cargo run -p ara-dev -- perf control --session <control-dir> --stop
# Attach an MCP client to the same warm player (stdio):
cargo run -p ara-dev -- perf mcp --session <control-dir>
```

`perf` builds the same four tool/player roots with the optimized `perf` profile,
then uses the machine GPU queue for the player. The profile inherits release,
enables incremental compilation and 256 codegen units, disables LTO and keeps
line tables. Dependencies remain at opt-level 3. Shipping and D26 confirmation
use `--profile release`; perf timings are iteration evidence, not a release pass.
The first cold build and initial content export are setup costs, not the warm loop.

Perf probes create an SDL window hidden from creation: `ara-dev perf` already passes
`--perf-output`, which implies this policy. Manual player probes can use `--offscreen`.
Rendering, submission and presentation continue; `player.json` reports
`presentation: "hiddenWindow"`. These timings cover the hidden render/present path;
desktop compositor latency and D27 acceptance require their own evidence.

The default scene is a once-exported frozen copy of authored Outpost from
`samples/outpost`, with the 144-tile scene from main. The production exporter
cooks it once into `target/ara-dev/perf/outpost`; subsequent runs never reimport
or repackage it. `--scene <export-folder>` selects another existing immutable pack.
To refresh content, choose another frozen export folder explicitly. Each report
records the pack's Git blob hash, camera, dimensions, preset, profile and active
shader cohorts; comparisons reject incompatible named baselines. Automatic
previous-run comparison starts a fresh baseline when identity changes.
Prepare sample art and generated kit geometry once through the sample's fetch/build
workflow (see `samples/README.md`), or supply an already exported `--scene`.
The command checks for missing source assets before building; sidecars alone cannot produce a pack.

The defaults are High, 2560x1440, 30 warm-up frames and 240 measured frames,
with validation disabled by the optimized profile. `--warmup`, `--frames`,
`--width` and `--height` override these. Reports contain GPU pass p50/p95/max,
CPU phase quantiles, frame time and FPS p50/p95/max, startup/first-frame timings,
build/total time, and previous-run deltas. JSON and command logs remain under
`target/ara-dev/perf/run-*`; `--json` prints the report and `--baseline <report>`
selects a named comparison. Keep-alive `--json` streams one compact JSON object per
completed window (NDJSON). `--no-build` reuses prior Cargo artifact paths;
`--player <exe>` can supply an explicit prebuilt player. The same selected package
set is kept after initial setup, preserving the build feature union.

`player.gpu_span_ms` is the whole presenter-device interval, including uploads,
view, particles, post, publication and inter-submission gaps. Overlap counts once.
`gpu_span_coverage` must cover every measured frame for D26 to gate it; unsupported
cross-queue clocks and failed frames stay unavailable. `gpu_span_samples_ms`
preserves each frame (null when unavailable). Pass begins follow leading barriers
at ALL_COMMANDS; `gpu_pass_sum_ms` is the exclusive view-pass sum for attribution,
not the whole-frame budget. KHR/EXT calibrated timestamps provide the common queue
clock. Native swapchain/UI copies and compositor latency are outside this span.

`player.gpu_other` retains scene-update, texture-upload, VFX, raster, fidelity and
publication timestamp quantiles, plus uploaded/resident texture bytes and pending
promotions. `accounted_ms`, `unaccounted_ms` and `outside_view_ms` are calculated
for each frame before quantiles; do not subtract or sum independently computed
medians to attribute a budget. Signed residuals remain visible. `raster_ms` covers
native geometry, look and shared post at every preset, including Low;
`fidelity_ms` is experimental RT only and stays zero for native raster. Native
terrain/foliage and VFX shared-post replay belong to the selected view's component.
`gpu_coverage.components` is complete when every frame supplies native components,
partial for mixed native/legacy windows, and unavailable for legacy-only windows.
Only the legacy diagnostic fallback has null components and outside-view/residual
values because its total omits view/landscape/VFX work. These fields also appear
in the terminal table and previous-run deltas.

`--keep-alive` holds the GPU lock while the player remains open. The control queue
supports fresh measurement windows, preset swaps, shader reload, and stop. A plain
`perf control --session <dir>` measures again. Raw player metrics are available
while it runs in `player.json`; the perf process publishes each completed window
and its comparison. Close the window or send stop to release the GPU for others.
The local MCP bridge exposes `perf_status`, `perf_measure` (optional `quality`),
`perf_reload_shaders`, and `perf_stop`; it neither rebuilds nor launches another player.
Ordinary exported player behavior and `ARA_RESULT` remain unchanged without perf flags.

Shaders already load cooked files at runtime. `ARA_SHADER_DIR=<workspace>` makes
player/editor resource selection explicitly prefer that workspace's
`crates/<package>/cooked` over executable-relative resources. A directory laid out
as `resources/<package>` or `<package>` works too. For an arbitrary single cohort:

```powershell
$env:ARA_SHADER_DIR='D:\shader-iteration\fidelity'
$env:ARA_SHADER_PACKAGE='ara-render-fidelity'
& "$env:CARGO_TARGET_DIR\perf\ara-shader-cook.exe" crates/ara-render-fidelity/shaders/pipelines.json $env:ARA_SHADER_DIR --watch
```

Set those environment variables before starting the player/editor. The cooker
watches sources/headers every 200 ms; successful cohorts publish a reload signal.
Compilation errors retain every last-good manifest and shader. The GPU worker
compiles changed native pipelines away from the frame thread, checks ABI/state
compatibility, publishes at a submission boundary, and retires old native handles
only after both queue timelines complete. Temporal fidelity histories reset on
publication. Shader epoch/cohort hashes and reload errors appear in metrics.
Layout/stage/render-state changes require a Rust rebuild. No Slang compiler is
linked into the player, and the watch command adds no build prerequisite.

Literal local include dependencies drive per-pipeline reuse; macro/system/module
includes conservatively invalidate the source tree. Unchanged SPIR-V is reused
across new cohort keys. A shader-only iteration needs neither Cargo nor a new
player process. Cook into an ignored scratch directory for experiments; tracked
cooked files are refreshed only deliberately.

### D26 scoreboard

`cargo run -p ara-dev -- d26` measures every D26/D27 release target in one pass and writes
`target/d26/<timestamp>/report.json` and a self-contained `report.html` (pass/fail per gate,
value and limit per row). It builds the perf roots plus `ara-editor` and `ara-samples` in one
feature resolution, freezes Outpost, Hangar and Showcase packs once per sample tree
(`target/d26/packs`), then runs, each step under the GPU lock:

- per scene, a cold High launch (fresh pipeline cache) and a warm launch per preset.
  GPU windows start after renderer readiness and warmup; CPU/hitch rows retain
  readiness, content/GI arrival and warmup frames in both launches;
- `ara-samples capture <project> <look> --quality all --reference 256` per scene (ꟻLIP/RMSE),
  or `--flip-from <capture-dir>` to reuse a sweep;
- the editor's `--verify-responsiveness` on a fresh `samples/outpost` copy with
  `--settle-imports after` (interactions during imports), then again on that copy with
  `--settle-imports before` (warm startup, Scene orbit, Play, interactions after imports).

A row without a measurement reads "not measured", never "pass". A readiness timeout
explicitly fails the GPU rows, retains `player.json` with the last renderer status,
and exits the player unsuccessfully. `--ready-timeout 120` bounds this wait (seconds).
Readiness requires native raster, GI enabled, complete publications/residency,
GI delivery/scatter/convergence and settled fades. Frozen packs have immutable
publications; this protocol does not certify a live editor's external import queue.
The ready snapshot is latched for the immutable pack, so ordinary in-flight frames
continue warmup. Quality/reload requests start a new readiness cycle. Presentation
failures after readiness explicitly fail the GPU rows and retain diagnostics.

Time to readiness, arrival/warmup GPU-span peaks and wall-frame peaks are reported
separately without gates. `player.arrival.gpu_passes` retains per-pass quantiles;
arrival coverage states how many frames have valid GPU spans. These are peaks of
the excluded frames, not isolated GI-only costs. CPU rows gate on
`run.cpu.work_ms` when its `work_coverage` covers every game frame, and hitch rows on
`run.cpu.hitches_over_16ms`. Only the first game frame is excluded from hitch counts;
the loading view remains in startup metrics. Keepalive gaps and later measurement
requests remain in `run.cpu`; Stop/quit flushes the final CPU totals while retaining
the last GPU window. `run.hitches` adds indexed host-phase/wait/readiness evidence
for the first 256 hitches (further details counted as omitted). Window-only `cpu` metrics remain in
the raw report, but do not gate whole-run CPU/hitches. Wall-clock CPU values only bound CPU work, so they pass
at or under the limit and are unmeasured above it. `--skip
player|flip|editor` leaves a section out; `--no-build` reuses the last d26 build;
`--profile release` confirms on D26's named profile. `--prepare` builds and freezes the
packs (`--refreeze` exports them again) without measuring anything, so it can run outside a
quiet window; the measured run then follows with `--no-build`. The report's footer defines
each metric.

For the same steady protocol in the inner loop, use
`cargo run -p ara-dev -- perf --wait-ready --ready-timeout 120`.
The player accepts `--perf-wait-ready --perf-ready-timeout-ms 120000` with
`--perf-output`; it stays hidden. `readiness.ready_frame` and `gpu_start_frame`
prove the boundary: readiness at frame 430 with 60 warmup frames starts sampling
at frame 491. A deadline failure never becomes a zero-cost GPU result.

Protocol verification on Windows/RTX 4070 SUPER (2026-10-26): a hidden 640×360
Low debug player on the frozen Outpost pack signalled readiness at frame 544;
two warmup frames placed the five GPU samples at frames 547–551. `run.cpu`
retained all 551 game frames. A separate 1 ms readiness deadline exited with code 2,
`completed: false`, zero GPU samples and the last renderer status. Both runs had
zero visible windows and no Vulkan validation errors, including timeout teardown.
Evidence: `target/ara-dev/import-speed/d26-proof-r3-{ready,timeout}/verification.json`
and `player.json`. These are functional protocol checks with debug validation,
not D26 timing acceptance results.

### Measured iteration loop

On 2026-10-24, both profiles were warmed with the same four package roots, four
build jobs and an independent D: target directory. Changing High's
`reflection_cutoff` from 0.60 to 0.61 in `ara-render-fidelity/src/presets.rs`, then
running the frozen 144-tile Outpost at 2560x1440, High, 30 warm-up and 240 measured
frames gave:

| Profile | Renderer rebuild | Rebuild plus native player run |
| --- | ---: | ---: |
| release | 28.03 s | 62.08 s |
| perf | 30.22 s | 61.94 s |

This sample shows no incremental compile advantage over release; both complete
the inner loop in about a minute. Cold compilation and the first asset import/
export are separate setup costs. The probe edit was restored afterwards.
GPU medians were 15.64 and 15.61 ms respectively, and first-frame times were
8.62 and 7.18 seconds: these measurements do not meet D26's release budget.
Local evidence is `target/ara-dev/perf-rebuild-results.json`, with build and player
logs beside it. A 60-frame run with a capture completed in 20.54 seconds once
the frozen pack was available.

On the final binary, editing `post.slang` in an isolated fidelity cohort produced
a new 60-frame window in 6.25 seconds: three offline pipelines cooked in 3.29
seconds and native replacement used 3.72 ms on the warm driver cache. The player
PID and executable hash stayed identical. An incompatible binding-layout probe
was rejected while retaining the last-good report/cohort; restoring the manifest
then reloading recovered successfully. A live High-to-Medium swap and the stdio
MCP status query passed on that same process. Evidence is
`target/ara-dev/perf-final-probe-results.json` and its before/after/rejection/
recovery reports. The final capture was visually inspected.

## Rust hotpatch (development)

`hotpatch` is an opt-in Cargo feature, off by default. The driver requires the
optional **dioxus-cli 0.8.0-alpha.3** (`dx`) with Ara's native Rust save fix;
normal builds/tests need no dx installation. Prepare it once with
`cargo run -p ara-dev -- hotpatch --install-dx`. This explicit setup downloads the
pinned upstream revision, applies the small checked-in native watcher patch, and
builds the optional tool with the engine's pinned Rust toolchain. `--plan` prints
the revision and destination without downloading or building. `ARA_DX` selects an
already prepared tool. A missing version or native-fix capability reports ARA7600.

```
cargo run -p ara-dev -- hotpatch --plan -- player
cargo run -p ara-dev -- hotpatch -- player --root <frozen-pack> --offscreen
cargo run -p ara-dev -- hotpatch -- editor <scratch-project>
```

The initial fat build is expensive. Subsequent edits use Subsecond's thin linking
and hot function table; the `hotpatch` profile favors compiler latency. The driver
isolates Cargo outputs, config and crash files under `<CARGO_TARGET_DIR>/ara-hotpatch`
(the checkout's `target/ara-hotpatch` when unset), uses
four BelowNormal build jobs on Windows (two jobs on Mac), and binds dx to loopback.
It forwards native arguments as data, without shell execution.

Windows uses an MSVC-shaped facade over the pinned rust-lld plus the installed
MSVC/SDK library environment. This is confined to the hotpatch command; ordinary
workspace linking is unchanged. Dev hosts reserve 16 MiB of stack because the
unoptimized renderer's graphics prewarm exceeds Windows' default 1 MiB reserve;
the paired companion receives the same setting through MSVC `editbin` after linking.
A guarded rustc facade also covers dx's direct
workspace replay, which bypasses Cargo's normal compiler wrappers. The editor is
launched through a bin-only tip target because dx skips a tip's own library
when it has both lib and bin targets (upstream #5778).

The editor lane first builds the editor and its matching Play companion in one
Cargo resolution and verifies both `--ray-tracing` values. Editor UI patches do
not patch that independently built companion. Use the player hotpatch lane for
player Rust iteration. Native hosts check the selected binary identity (including
Cargo's hashed filename), so a helper cannot register against its parent's patch
baseline. Each independent binary needs its own baseline and driver port.

The optional native watcher patch bypasses RSX diffing only when Ara sets
`ARA_NATIVE_RUST_PATCH`. Stock alpha.3 can silently miss an undo/redo save after a
multi-file patch because its file snapshots become stale. The prepared CLI
replays every native Rust save, including saves queued during compilation.
Native capability v2 also drains queued saves after a failed compile; upstream
only drains them after a successful build.

The CLI prerelease is required for its fix to uncaptured workspace replay (#5903).
Ara keeps Bevy's Subsecond 0.7.10 runtime; the prerelease's patch-table schema is
unchanged. On Windows the facade also quotes dx's raw response capture and rewrites
only the generated `__chkstk` COFF thunk to preserve its RAX stack-size argument.
Without that workaround, patching a 47,936-byte render recording frame overflows
the stack. No loaded machine code is modified by Ara's linker workaround.

One host owns publication. The websocket worker checks PID/build identity and
source compatibility, then queues a patch. Publication happens between frames,
only after all hot ECS/pass scopes have finished; a busy scope defers the patch.
GPU work already submitted keeps its existing resources and queue-timeline
retirement. No GPU idle wait or resource migration is added by code publication.
The editor invalidates retained panel markup on apply. ARA7601 reports actual
publication, ARA7602 reports rejection/rebuild, and status retains generation,
last patch time and the rejection reason.
DX's `compileMs` covers the tip build, so it is not edit-to-frame latency; the
measurements below also include dependency replay, linking and delivery.

The initial source contract is deliberately conservative:

| Edit | Action |
| --- | --- |
| Synchronous body, unchanged signature/data definitions | Live at its hot entry point |
| Type/field/enum/signature, static/TLS, build.rs, manifest or dependency change | Needs rebuild |
| Reviewed render `record_frame`, fidelity `record_post_process`, editor `draw_panel`, including synchronous local closures | Live; the hot scope lasts through all local calls |
| Const/async/generic/trait-implementation, retained or unreviewed closure-bearing body, impl Trait | Needs rebuild |
| Resource creation/retirement body or unknown macro-bearing body | Needs rebuild |
| Shader binding layout | Existing shader cohort compatibility checks apply |

Old closure environments, vtables and futures are not re-instanced. Rebuild and
restart after ARA7602 to establish a new baseline. Keep source inputs stable while
a patch is compiling, just as for an ordinary Cargo build. No arbitrary ABI or
resource-layout change is accepted as a live migration.

Bevy ECS 0.20's experimental hotpatching dispatch is enabled for ara-sim. A patch
changes `HotPatchChanges` before the next tick so both ordinary and exclusive
systems refresh their function pointers. The process is explicitly tainted:
simulation hashes and replay/snapshot headers flag it, and deterministic replay,
checkpoint restore and network continuation refuse it with ARA7603. Local
development simulation continues. Patched portable artifacts also use format 2,
which older readers refuse; unpatched artifacts retain format 1. Release hosts fail compilation with hotpatch
enabled, and ara-export rejects a marked development executable both when making
and when unpacking a runtime template.

`ara-dev lab` can call `hotpatch::available` before choosing its Rust reload
strategy; a missing/incompatible tool selects its existing warm-restart path.
ARA7602 requires rebuilding/restarting rather than retrying an incompatible patch.
No lab state or file-control protocol is owned by the hotpatch module.

For a windowless real-renderer proof without game imports:

```
cargo run -p ara-dev -- hotpatch -- render-probe --output target/hotpatch-proof
cargo run -p ara-dev -- hotpatch -- render-probe --fidelity --output target/fidelity-hotpatch-proof
cargo run -p ara-dev -- hotpatch -- sim-probe --output target/sim-hotpatch-proof
```

The probe acquires the machine GPU queue after its initial build, then runs the
renderer beneath that guard for correct load attribution. Collect generation
captures/status and create `<output>/stop` to finish and release the queue.
`--start-file <path>` optionally defers device creation for measurement coordination.
The render probe uses actual ara-render recording; `--fidelity` uses an RT FidelityView
and its actual graph-building recording function. The sim probe keeps a real ECS
entity/query/system alive and verifies replay, restore and network refusal after a
patch. Captures prove behavior, not D26 performance.

Measured evidence and current limitations: [S11](../../spikes/s11-rust-hotpatch/RESULT.md).
Its initial 0.39 s Mac result is a minimal loop measurement, not editor latency.
Windows' actual fidelity/system/panel first edits measured 21.87 / 3.85 / 20.59 s;
repeated edits varied up to 34.57 s. This is currently an experimental functional
lane, above the 2–3 s latency target. Mac ECS/editor and the lab strategy adapter
remain pending.

## Gate usage

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
package; root documentation and AGENTS.md need hygiene and formatting.
Changes to `release/licenses/` select the `ara-release` inventory/packaging tests;
an absent packager retains the conservative workspace fallback. Crate
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

An affected gate has a 2400-second execution budget, including metadata.
Reported Cargo package-cache lock waits are excluded and carried forward to later
phases; command details record excluded waits, while reported seconds remain wall time.
Build-directory/shared-lane waits still count. Timeout returns failure with the
command log and terminates its process tree (a kill-on-close Windows job).
It cannot waive an unfinished test.
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
(default 20 GiB). In a Rust checkout it also inspects effective Cargo target/build
directories, including global configuration, and flags another checkout's target.
Inspection uses read-only Cargo metadata with a ten-second deadline. `check` and
`merge` reject explicit foreign outputs before building or changing merge state;
wrapped GPU commands use the same output guard. Use the Windows entry script to
keep the tool's bootstrap itself independent of global Cargo target settings.
Cleanup lists registered worktrees and recursively measures each
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

Repeated process-test diagnostics found correctly received authority state but
stale owner predictions. No-input-delay mode permits 100 predicted ticks, while
the default manager retained only 20 rollback ticks. Client startup now aligns
history retention with the existing prediction budget. A fixed-clock regression
with 250 ms one-way delay failed before the fix in 1.56 seconds (no checkpoints or
rollback), then reconciled and replayed successfully in 1.06 seconds. Reconciliation
uses native authority/history data; no test snapshot repairs or assertions were added
to the runtime. Timeout reports retain mismatched state/view details for diagnosis.

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

After merging main's quality shaders, the required cook regenerated nine baseline
renderer pipelines from the merged baseline.slang. Fidelity and VFX cohorts passed
the same cooker with matching artifacts reused. No cooked conflict was resolved
by selecting one side.

Player fixtures reuse the gate's current import-worker artifact. Standalone
player tests still build the worker with --locked and read its actual executable
path from Cargo's messages. The fixture assertions and importer-version checks
are unchanged. A cross-crate CLI fixture verifies freshness after a peer edit
with separate build/intermediate directories; test harnesses are never exported
as regular binaries.

The generated-binding check treated every schema-blittable component as a native
Rust view, while the runtime binds only explicitly validated engine components.
Generation and runtime startup now share EngineBindings::builtin; a regression
checks actual Rust offsets and ABI hash agreement. Schema-only render components
do not enter the native ABI merely because their canonical layout is blittable.
Changes under dotnet/ select ara-script and
its reverse dependency closure, with conservative workspace fallback if that host
is absent. A real metadata/Git fixture verifies both paths.
