# S11: Rust hotpatch checkpoints (2026-10-27)

**Preview-4 latency pass:** Windows fidelity recording has a 2.476 s warm median;
the small real-ECS system probe has a 2.149 s default median (1.388 s with a
targeted 16-CGU setting). Editor default warm median is 15.205 s; lab publication
verification is in progress. This remains an off-by-default experimental dev feature, with
material shared-disk tails. The later sections contain stage timings and lever
comparisons; the original functional checkpoint below is retained as history.

| Windows native CLI v2 | First body edit | Repeat body edit | Restore/recovery |
| --- | ---: | ---: | ---: |
| Fidelity render pass | 21.87 s | 4.33 s | 6.67 / 11.37 s |
| Bevy ECS system | 3.85 s | 34.57 s | 5.22 / 6.25 s |
| Editor panel | 20.59 s | 33.06 s | 29.47 / 13.18 s |

These are save-to-observed-output wall times, including workspace replay,
compilation, linking, delivery and output sampling. They are not DX's shorter
tip-only compile timer. Each row kept its process and state alive. Mac raster
rendering previously passed around 1 s. The new Mac ECS confirmation measures a
0.494 s warm median on AC; the editor confirmation is in progress.

## Live/rebuild contract

| Change | Ara action |
| --- | --- |
| Synchronous body behind a hot render/system/panel entry, with unchanged signature and data definitions | Publish at the next quiescent frame boundary; system runs it on the next tick |
| Existing ECS entities/components/resources and GPU handles | Preserve them; existing queue-timeline retirement continues |
| Fresh synchronous closures in reviewed `record_frame`, `record_post_process`, `draw_panel` entries | Live with the enclosing hot scope |
| Type/field/enum/signature, reflected schema, static/TLS initializer, manifests, dependencies or native source | Reject with needs rebuild; restart to establish a new baseline |
| Async/futures, generic or trait-implementation bodies, impl Trait, retained/unreviewed closures or vtables | Rebuild; retained objects are not re-instanced |
| GPU resource creation/retirement or resource layout changes | Rebuild; no live resource migration |
| External dylib/plugin ABI or code | Rebuild; Subsecond patch libraries themselves remain loaded for process lifetime |
| Shader-only edits | Existing independent shader reload/cohort checks |
| Replay, snapshot restore or network continuation after any applied Rust patch | Refuse with ARA7603; process/header/hash are marked, including legacy-reader format rejection |
| Editor's separately built Play companion | Keeps built code; player Rust iteration uses its own driver/baseline |
| Shipping/export executable | Feature is off by default, release compilation rejects runtime opt-in, export rejects the marker |

The lab adapter is implemented against `ReloadStrategy`; its real resident-player
verification is still pending. Ordinary lab defaults to rebuild plus warm restart.
Source inputs must remain stable while the compiler reads them. The source guard
is a conservative compatibility policy, not arbitrary Rust ABI/state migration.

## Minimal loop spike

**Works on Windows and Apple silicon. Windows latency target is not yet met.**
This windowless fixed-frame probe is not proof of Ara render/editor integration.
The process ID, frame counter and start Instant survived a function-body edit.

| Host / toolchain | Driver | Edit to changed frame | Result |
| --- | --- | ---: | --- |
| Windows 11, 7800X3D, Rust 1.99.1, BelowNormal, 4 jobs | dx 0.7.10 + rust-lld facade | 11,782.63 ms | Same PID 26888, body 2 at frame 1710 |
| macOS 27, M5 Pro, Rust 1.99.1, nice 10, 2 jobs, AC | dx 0.7.10 | 392.26 ms | Same PID 30321 |

Windows dx reported 10,650 ms: 8,987 ms compiling, 522 ms caching objects,
1,126 ms linking. The busy shared PC also had multi-second gaps between frames.
These are observed wall times, not uncontended performance claims. Output was
sampled every 30 frames; observed latency includes that interval. Mac includes
an approximately 20 ms log poll. Neither probe created a visible window or GPU work.
Both drivers/processes were stopped.

Evidence: `target/hotpatch-research/spike-windows-{facade.log,facade-measurement.json}`;
Mac scratch: `/Users/islam/projects/ara-hotpatch-spike/{mac.log,measurement.json}`.

## Windows findings

- Stock Ara `linker = "rust-lld.exe"` fails under dx: rustc finds its sysroot and
  supplies `-flavor link`, but dx invokes the linker directly.
- Absolute MSVC `link.exe` needs the SDK `LIB` environment. A later fat archive
  failed with LNK1136. An earlier closure probe reported success but kept the old
  body; that is not a pass.
- Direct `lld-link.exe` receives `-flavor link` from rustc here, which its frontend
  rejects. The legacy linker-flavor alias did not fix it.
- `linker.rs`, compiled as `link.exe`, forwards to pinned `rust-lld.exe` with
  `-flavor link` exactly once. With MSVC/SDK `LIB`, the edited function ran through
  `HotFn::current(tick).call(...)`.
- dx is optional. Checksum-verified prebuilt binaries lived under target, not a
  global installation. Windows ZIP SHA-256:
  `45eb4f87b7f86fdba8508ee7e0ef1f9e13cf8f2ffcc6a2d5858d0e8ea7dd1531`.
  Mac tar SHA-256: `342c92bc33dca60dadd725ed5e249923f049ae2e1e50b7477f4968d56695b425`.

## Primary sources (inspected 2026-10-27)

The initial minimal spike tested subsecond and dx 0.7.10. Latest stable dx is
0.7.10; the real-render integration below additionally tests 0.8.0-alpha.3.
Source reads included published
subsecond 0.7.10, dx's `build/{link,request,builder}.rs`, and installed Bevy 0.20.

- [Subsecond API and limits](https://docs.rs/subsecond/0.7.10/subsecond/): jump-table
  detours and HotFn; static initializers do not rerun; layouts are not migrated.
  Retained trait objects, futures, closures and function pointers can keep old code.
- [Runtime source](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html):
  apply_patch uses libloading on Windows/Unix, rebases against the ASLR sentinel,
  publishes the table globally, and retains code libraries. Ara must queue it for
  a quiescent boundary instead of applying from the websocket worker.
- [Bevy function systems](https://github.com/bevyengine/bevy/blob/main/crates/bevy_ecs/src/system/function_system.rs)
  and [plugin](https://github.com/bevyengine/bevy/blob/main/crates/bevy_app/src/hotpatch.rs):
  the hotpatching feature already dispatches ordinary/exclusive systems through
  HotFn. Changed HotPatchChanges refreshes pointers; HotPatched notifies consumers.
  The ECS World/resources survive; arbitrary layout migrations are not provided.
  Ara already has this in Bevy 0.20; no fork or Bevy App dependency is needed.
- [Workspace/TLS PR #5291](https://github.com/DioxusLabs/dioxus/pull/5291): workspace
  replay and dynamic TLS fixups exist despite the runtime docs still saying tip
  crate only. Multi-crate behavior needs explicit verification.
- [Lib+bin issue #5778](https://github.com/DioxusLabs/dioxus/issues/5778): confirmed
  0.7.10 replay-order code excludes the tip package. The editor has lib+bin; use
  a separate bin-only launcher rather than silently accepting an empty patch.
- [Windows symbols #5626](https://github.com/DioxusLabs/dioxus/issues/5626): v0
  mangling fixes reported symbol mismatches; use it consistently in this lane.
- [Release notes](https://github.com/DioxusLabs/dioxus/releases/tag/v0.7.10) and
  [linker fix #5242](https://github.com/DioxusLabs/dioxus/pull/5242): Windows linker
  handling differs from normal cargo. Cargo success alone is insufficient.

## Initial integration policy

| Change | Policy |
| --- | --- |
| Synchronous body, stable signature and stored data | Hot-callable; prove each real dispatch path |
| Struct/enum/alias/trait/impl signature; CPU-GPU layout | Needs rebuild |
| Generic or impl Trait body | Needs rebuild initially |
| Closure-bearing function or async state machine | Needs rebuild; retained environments are not migrated |
| Statics/TLS/initializers; build.rs; manifests/dependencies | Needs rebuild |
| Shader bindings/resources | Existing shader cohort checks and timeline retirement remain authoritative |
| External native code/dylibs | Needs rebuild |
| Replay or network continuation after a Rust patch | Explicitly tainted; refuse deterministic continuation |

Next: queued frame-boundary owner, compatibility rejection, session taint and
export exclusion; then dependency-crate and actual render/sim/editor proof.
Those six Windows/Mac measurements are **not measured** at this checkpoint.

## Windows native render integration (2026-10-27)

Real `Renderer::new_native`, instanced triangle, LitWireframe, 320x180, fixed
camera/tick/seed. Edited `record_wire_composite`'s `words[5]` assignment from
`u32::from(lit)` to `u32::from(!lit)`. Visually inspected captures change the
triangle's yellow wire edge to blue, then restore it exactly. No window was shown.

| Run / operation | Edit to matching rendered frame | Result |
| --- | ---: | --- |
| R8 first body edit | 3,861.52 ms | Same PID 32252, changed PNG |
| R8 frame-layout edit | 336.25 ms to rejection | No apply/restart; frames continued |
| R8 restore after rejection | 3,692.92 ms | Original PNG restored |
| R9 first body edit | 4,763.77 ms | Same PID 36132, generation 1 |
| R9 frame-layout edit | 452.21 ms to rejection | Generation unchanged; frames continued |
| R9 restore after rejection | 5,317.39 ms | Original PNG restored |
| R9 rapid repeated body edit | 8,938.49 ms | Changed PNG, generation 4 |
| R9 restore | 5,647.41 ms | Original PNG, generation 5; clean GPU exit |

R8's repeat check originally stopped on an older, already-in-flight generation;
that was a measurement-harness error, not a successful repeated-edit result.
R9 waits for the matching image, exposing the additional queued-build latency.
The complete R9 sequence passes. This is functional evidence, **above the 2–3 s
target**, not an uncontended latency or full-player fidelity benchmark.

Evidence: `target/hotpatch-proof/windows-render-r{8,9}/` (measurements/status/PNGs),
`target/hotpatch-research/render-driver-r{8,9}.log`. GPU admission/load attribution
was valid; R9 report: `target/ara-dev/gpu-load/20720-1793121074274.json`.
The manual opt-in `measure-render.ps1` edits/restores the two source files in this
worktree; never run it while another writer owns them. It is not a Cargo test.

Three upstream/tooling blockers required changes:

1. Stable dx 0.7.10 replays every reverse workspace consumer, including unbuilt
   siblings, failing with `Missing rustc args for replay: 'ara_anim_capture'`.
   [PR #5903](https://github.com/DioxusLabs/dioxus/pull/5903) fixes this; use
   [dx 0.8.0-alpha.3](https://github.com/DioxusLabs/dioxus/releases/tag/v0.8.0-alpha.3).
   Its `subsecond-types` schema is byte-for-byte identical to 0.7.10; native
   apply_patch differs only in explicit unsafe blocks. Keep one 0.7.10 runtime
   table shared with Bevy, rather than introducing an independent 0.8 table.
2. dx's raw `link_args.json` capture is not a quoted Windows response file.
   Paths under Program Files failed. Ara's scoped linker facade quotes it; a
   regression test invokes actual rust-lld against the failing and fixed inputs.
3. [The CLI's Windows x64 text thunk](https://github.com/DioxusLabs/dioxus/blob/v0.8.0-alpha.3/packages/cli/src/build/patch.rs)
   uses `movabs rax,address; jmp rax`, destroying __chkstk's RAX input. CDB caught
   the failure in the prologue of a 47,936-byte `Renderer::record_frame` stack
   allocation; disassembly showed the size replaced by the code address. Ara
   rewrites only that generated COFF helper before linking to a register-preserving
   RIP-indirect jump. It does not rewrite loaded executable memory. The original
   overflow reproduces; the same real renderer then passes repeated edits.

Additional integration limits: dx metadata must see dependency crates without
optional feature activation; the proof and editor launcher use normal path
dependencies with opt-in runtime features. The lib+bin tip workaround remains.
Mac real render, simulation dispatch and editor-panel measurements remain pending.

### First Mac native-render result and remaining CLI cache blocker

The same native scene/pass edit runs under Metal API/shader validation on the M5
Pro (AC, nice 10, two compiler jobs, windowless): **1,044.87 ms** to a changed PNG
in PID 98064. Layout rejection took **200.94 ms**, frames continued, and restoring
the layout/body restored the original PNG in **1,036.48 ms**. Captures were visually
inspected. This is a reload-latency/functional proof; the Windows per-process GPU
load sampler is unavailable on Mac, so no guarded GPU-performance claim is made.

The rapid repeat then timed out with generation 2 still running. Inspection of
dx 0.8.0-alpha.3 `serve/runner.rs` identifies a source-cache bug: RSX diffing exits
on the first Rust body difference in a file batch, leaving other file snapshots
stale. After a two-file recovery, saving a body matching that stale snapshot can
be silently ignored. This blocked the initial checkpoint. A small optional
CLI patch was verified next: native-engine mode replays Rust saves directly,
including queued saves, bypassing RSX diffing. Normal Ara builds still need no dx.

Mac evidence: `target/hotpatch-research/mac-render-r1{.log,-measurements.json}`,
before/after PNGs there, and the isolated remote worktree's
`target/hotpatch-proof/mac-render-r1/`. Both probe and original driver were stopped.
Simulation/editor live-patch measurements remain pending on both platforms.

### Prepared native CLI: repeat-save recovery verified

The native-only CLI patch fixes the reproduced stale RSX snapshot issue. It does
not change normal Dioxus operation. `ara-dev hotpatch --install-dx` builds the
optional pinned tool; launch checks both its base version and native capability.
Normal workspace builds do not execute this setup.

| Real native renderer, one unchanged PID | Windows (D: build disk) | macOS (AC) |
| --- | ---: | ---: |
| First edited body to changed GPU capture | 4,137.52 ms | 1,029.68 ms |
| Layout rejection, old frames continue | 341.91 ms | 188.66 ms |
| Restore compatible layout/body | 4,213.75 ms | 971.69 ms |
| Rapid repeat of edited body | 7,205.64 ms | 975.71 ms |
| Restore original body again | 4,305.09 ms | 912.34 ms |

Windows PID 42784 and Mac PID 11760 survived the entire sequence. Output captures
changed and returned to the original hashes; captures were visually inspected.
Windows ran under the machine GPU guard; Mac used Metal validation, with no
per-process GPU-performance claim. Both drivers were stopped and sources restored.
Evidence: `target/hotpatch-research/{render-driver-r10,measure-render-r10,mac-render-r2,mac-measure-render-r2}.log`
and per-generation captures/measurements in `target/hotpatch-proof/`.

### Fidelity recording and failed-build queue recovery

The actual `FidelityView::record_frame` body patched on Windows in PID 48068:
**11,234.79 ms** from an exposure edit to a visibly changed GPU capture. Changing
`GpuFrame.inverse_vp` from `Mat4` to `[Mat4; 2]` was rejected in **523.13 ms**;
the old frames continued. These are shared-PC, BelowNormal, D: observations.
The body edit's DX breakdown was 6,697 ms workspace replay, 2,645 ms tip compile,
495 ms object caching and 585 ms linking. DX's shorter `compileMs`/summary omits
workspace replay. Source validation/delivery/publication accounted for another
approximately 0.65 s.

The compatible save made during the rejected compile remained queued. Upstream
`serve/mod.rs` drains pending changes after `BuildReady`, but not `BuildFailed`.
Ara's native-only v2 patch drains that failure queue too. Its optional installer
checks the exact pinned source diff before building. The later v2 sequence below
verified recovery. Initial evidence:
`target/hotpatch-research/{fidelity-driver-r3,measure-fidelity-r3}.log` and
`target/hotpatch-proof/windows-fidelity-r3/` (both captures visually inspected).

The source guard permits literal-producing macros and fresh synchronous closures
inside the reviewed recording and panel entries. A test uses the actual fidelity
source to keep that entry patchable. Declarations, persisted environments and
resource creation/retirement remain rebuild boundaries. DX's hash-suffixed native
filenames are accepted by exact bin identity; helpers/Play companions cannot
register against an editor's patch table.

### Windows native v2 fidelity sequence

One PID **50652** survived all five phases, with captures changing and returning
to the exact baseline hash. The before, edited and restored captures were visually
inspected. Compatible saves made during the rejected compile were processed.

| Phase | Edit to observed frame/event |
| --- | ---: |
| First exposure body edit | 21,872.43 ms |
| Incompatible GPU-frame layout rejected | 484.47 ms |
| Compatible recovery to original output | 6,665.72 ms |
| Repeat body edit | 4,326.02 ms |
| Restore original output again | 11,369.74 ms |

Evidence: `target/hotpatch-proof/windows-fidelity-v2-r1/measurements.json`,
`target/hotpatch-research/fidelity-v2-r1-{driver.out,driver.err,measure}.log`.
The outer PowerShell harness miscounted its decoded JSON array after collecting
all five measurements and stopped the driver; the array handling was corrected
before the ECS/editor runs. This did not invalidate the captured live sequence,
but this run is not evidence of a clean guard/device shutdown.

### Windows ECS dispatch and persisted state

A real `Session` and named Bevy system changed a registered Transform increment
from 1 to 2, then back, twice in PID **33136**. Entity **4294967293** survived;
its counter continued forward rather than resetting.

| Phase | Edit to observed tick |
| --- | ---: |
| First system body edit | 3,846.69 ms |
| Restore original increment | 5,216.41 ms |
| Repeat body edit | 34,567.61 ms |
| Restore again | 6,245.00 ms |

The outlier was in DX's compile phase (**33,203 ms**); publication remained
approximately 0.4 s after delivery. These are shared-PC observations, not a
quiet-machine latency claim. The session's serialized replay flag survived a
binary round trip. Patched replay and snapshot headers use format 2, so legacy
readers refuse them too; ordinary artifacts retain format 1. Replay, checkpoint
restore and adapter-driven network continuation refused, without changing the
tick, entity or component value. The final original body still left the process
explicitly tainted.

Evidence: `target/hotpatch-proof/windows-sim-v2-r4/measurements.json` and
`target/hotpatch-research/sim-v2-r4-*`. A harness cleanup check encountered an
already-exiting process with no module path after these four measurements; the
read-only wait was corrected before the editor run. Functional measurements
preceded that cleanup error.

The initial ECS fixture queried Bevy's internal resource entities too; it now
selects the entity containing Transform. Its fast startup also needs the native
websocket/ASLR handshake to settle before timing the first save.

### External build-directory cache workaround

With Cargo outputs on D: outside the checkout, DX can treat every rlib as an
external/compiler library and write a zero-byte `libdeps-*.a`. The first link
skips that archive, but the cached path tests only existence and force-loads it,
causing LLD's `unknown file type`. Ara's linker facade ignores only that known
zero-byte generated archive (including its `/WHOLEARCHIVE:` argument); DX still
passes the original compiler rlibs. A regression test links through the real
pinned LLD and verifies that nonempty and unrelated archives remain untouched.
This workaround does not change normal workspace linking or delete evidence.

### Windows editor panel and paired Play startup

The actual editor `draw_panel` changed Console to a visible `RUST PATCH LIVE`
panel, restored Console, repeated the edit and restored again. The editor PID
**28144** and protocol session **ses_ayaq18bw** stayed unchanged throughout.
Production layout capture detected the marker; GPU captures confirmed it visually.
The baseline, changed and restored screenshots were inspected. No window was shown.

| Phase | Edit to observed layout and GPU capture |
| --- | ---: |
| First panel body edit | 20,593.90 ms |
| Restore Console | 29,474.36 ms |
| Repeat panel body edit | 33,060.12 ms |
| Restore Console again | 13,178.83 ms |

The editor exited gracefully, and the Windows GPU guard reported valid. Evidence:
`target/hotpatch-proof/windows-editor-project-v2-r4/.ara/hotpatch-proof/measurements.json`
and `target/hotpatch-research/editor-v2-r4-*`. Initial startup was expensive:
12m23s for the matching editor/player Cargo build, then approximately 11m for the
DX fat build. These are separate from the edit timings.

Capture inspection exposed stack overflows in the separately built, opt-level 0
Play companion's graphics prewarm. A direct hidden prewarm reproduced this with
the default 1 MiB Windows executable stack. Changing only its reserve to 16 MiB
completed prewarm and reached the expected owner connection error. The dev-only
linker facade now supplies the same reserve to DX hosts, and standard MSVC
`editbin` sets it on the paired companion after linking. A real-LLD/editbin test
verifies the PE stack reserve. Normal workspace and shipping linking stay unchanged.

The original editor/pool startup was rerun with that companion: editor PID
**43752**, `play.status.pool.headless = 1`, still ready ten seconds later, exactly
one companion log and no overflow/respawn. Editor and companion exited normally;
GPU attribution was valid. Evidence:
`target/hotpatch-proof/windows-editor-stack-fixed/{pool-ready.json,pool-still-ready.json,driver.err.log}`.
The local harness initially misread its single GPU-report line as a character;
read-only parsing afterward confirmed the valid report. This was a report-reader
error after clean shutdown, not a missing readiness or cleanup check.

### Dispatch overhead and checkpoint validation

Windows `overhead` example, current dev code, BelowNormal, five samples of two
million calls: median direct body **1.039 ns**, hot call **4.904 ns**, idle boundary
**2.507 ns**. Single thread and empty patch table; this does not measure populated
table lookup or parallel-system contention. Evidence:
`target/hotpatch-research/overhead-windows-current.log`. Default player feature
resolution contains neither ara-hotpatch nor subsecond; there is no idle poll in
that build.

Workspace fmt, touched-crate feature-on Clippy with `-D warnings`, focused tests,
and diagnostic catalog coverage pass. An initial catalog test caught duplicate
ARA7603 declarations; the warning now has one canonical owner. Explicit hidden
player GPU startup and error-retirement tests also pass. No visible test window.
Checkpoint logs are `target/hotpatch-research/stage2-*`.

The next checkpoint adds native CLI installation/capability v2, external D: cache
handling, editor companion pairing, the three dispatch proofs, and persisted
determinism rejection. Portable changed-crate Clippy/tests and diagnostic coverage
passed on m6u; Windows repeated native facade/runtime/tooling Clippy/tests and
catalog coverage. The stack correction additionally reruns ara-dev's focused
checks, including the real MSVC editbin regression. Logs:
`target/hotpatch-research/{linux-stage3f,linux-editor-stage3,windows-stage3-*}.log`.
One ara-dev fixture run found the machine's installed sccache instead of exercising
its intended missing-cache path. The final run uses the pinned compiler's bin
directory without the user Cargo tool directory, as in the preceding native gate.

## Windows latency investigation (2026-10-28, in progress)

The accepted functional checkpoint is `b7f59228`. The next pass keeps its
body/layout policy, frame publication, persisted ECS state and determinism guards.
All builds use D:, four BelowNormal jobs and hidden GPU hosts. Other agents and
the integration gate are active; these are observed development latencies, not
isolated CPU benchmarks. Mac ECS/editor measurements remain pending behind the
preview verification agent's exclusive Mac slot.

Manual `measure-latency.ps1` alternates one body and its exact original, verifies
the same PID and actual output, and restores sources in `finally`. Each compiler
configuration has one first sample followed by three warm samples. First samples
are retained separately; they are not part of the warm median. `analyze-latency.ps1`
correlates file-watch, rustc, linker, transport and frame-application timestamps.
Linker spans nested inside rustc are subtracted from compiler wall time to avoid
double counting. Rustc CPU time helps identify scheduling/IO delay. Optional
tracing adds no filesystem access to an idle frame check.

### Causes established so far

1. Windows saves generate repeated write/data notifications. The native v2 fix
   queued them even when the content was identical. In the fresh ECS baseline,
   four saves produced six applied generations. Deduplication produced exactly
   twenty generations for twenty subsequent measured saves.
2. With Ara's custom lld facade, upstream `BuildRequest::cargo_build` supplies a
   custom linker even for Fat/Thin mode. This makes rustc perform an executable
   link before dx's separate patch link. Native mode now uses upstream's no-link
   interception for the first step and retains the configured lld for the real
   link. Traces distinguish the two; this is not a switch away from lld.
3. Dependency replay uses `Command::new("rustc")` with Cargo's captured PATH.
   On Windows that selected the real compiler ahead of Ara's facade. Native
   replay now selects explicit `RUSTC`; otherwise compatibility checks, per-crate
   experiments and changed-crate traces can be bypassed. Earlier render records
   with no changed-crate trace cannot establish that those experiments applied.
   The Cargo workspace wrapper also honors its explicit compiler argument in
   native mode. Cached invocations restore the current Ara contract/trace/settings
   environment, rather than a previous fat build's session paths. Native build
   input changes report ARA7602 instead of letting dx restart the process itself;
   the lab owns the explicit rebuild and warm restart decision.
4. Incremental compilation was already enabled (`-C incremental`, 256 codegen
   units, line-table debug info). The old 33.203 s ECS outlier rebuilt only its
   small tip crate; it was not evidence of recompiling large ara-sim consumers.
   The old 33.060 s editor sample included a queued stale build followed by the
   requested build. Shared IO/CPU delays remain material: one new run overlapped
   disk cleanup and spent ~16 s wall time for ~0.75 s rustc CPU. That entire run
   is excluded from speed comparisons, not silently folded into the baseline.

Primary implementation evidence is pinned dx revision
`cb27832398e8a3fa30a0fc8d6917c6d8a86fa71a`, `packages/cli/src/build/{request,link,patch}.rs`,
and the reviewed embedded `dx-native-watch.patch`. The official
[Cargo profile reference](https://doc.rust-lang.org/cargo/reference/profiles.html)
documents the incremental and codegen-unit controls. The optional
[Cranelift backend](https://github.com/rust-lang/rust/blob/main/compiler/rustc_codegen_cranelift/Readme.md)
needs a separately selected nightly/component; its Windows/macOS unwinding
limitations require an aborting isolated profile. Stable LLVM remains the default.

### Completed ECS lever comparison

This named system runs in a real Bevy/Ara Session but lives in the small probe
crate. These numbers do not claim that editing ara-sim itself costs the same.

| Configuration | Warm median | Warm range | Evidence |
| --- | ---: | ---: | --- |
| Duplicate notifications enabled, old double link | 6.079 s | 5.056–7.704 s | latency-before-r3-sim |
| Deduplicated, existing incremental/256 CGUs | 3.056 s | 3.032–3.194 s | latency-after-r1-sim |
| Deduplicated, 16 CGUs for the system probe | 2.576 s | 2.377–2.625 s | same run |
| Deduplicated, 64 CGUs | 3.021 s | 2.764–3.030 s | same run |
| Deduplicated, incremental disabled | 2.926 s | 2.844–3.002 s | same run |
| Deduplicated, lld threads=4 | 2.929 s | 2.923–3.030 s | same run |

Each evidence directory is under `target/hotpatch-proof/` and contains raw
latency, stage, trace and driver records. Twenty edits retained PID 39964, the
same entity and forward-moving component state. The initial no-redundant-link
ECS run retained PID 24024: its 16-CGU warm median was 2.395 s, but a 4.743 s
outlier prevents claiming a reliable 3 s ceiling under shared load. Do not
change engine-wide codegen settings from this small-crate result.

Per-stage accounting below uses the actual median-total warm sample from each
configuration, so columns refer to one edit and add to its observed total. The
compile column includes the compiler-facade guard. The dx link column includes
the redundant nested link in these pre-link-fix runs. The remaining driver column
includes Cargo/cache handling and symbol-table construction/delivery.

| ECS probe | Watch | Compile + guard | dx links | Other driver | Receiver validation | Boundary + apply | Observed output | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Before | 2 ms | 1,685 ms | 3,363 ms | 336 ms | 567 ms | 22 ms | 105 ms | 6,079 ms |
| Deduplicated | 1 ms | 919 ms | 1,400 ms | 179 ms | 504 ms | 5 ms | 48 ms | 3,056 ms |
| Plus 16 CGUs | 2 ms | 715 ms | 1,262 ms | 137 ms | 428 ms | 17 ms | 16 ms | 2,576 ms |

Disabling Windows PDB output is rejected as a setup error: dx reads global
symbols from the fat and patch PDBs to construct its jump table. It is not a
supported linker optimization. No engine crate splits are included in this pass.

### Actual fidelity recording, explicit replay and one real link

PID 34232 survived 28 alternating exposure edits in the actual
`ara_render_fidelity::FidelityView::record_frame` path. Every sample checked the
rendered output, not just an applied event. One additional generation exposed a
remaining watcher duplicate; its source-snapshot correction is being validated
separately before selecting defaults. The initial fat build took about six
minutes after changing the compiler facade; that is excluded from edit latency.

| Configuration (changed crate unless specified) | First sample | Warm median | Warm range |
| --- | ---: | ---: | ---: |
| Incremental, 256 CGUs | 6.448 s | 4.083 s | 3.583–4.809 s |
| 16 CGUs | 5.059 s | 5.063 s | 4.007–7.139 s |
| Host only, 16 CGUs | 4.000 s | 3.259 s | 3.002–4.423 s |
| Changed crate and host, 16 CGUs | 3.286 s | 2.748 s | 2.221–4.723 s |
| 64 CGUs | 2.764 s | 2.325 s | 2.144–2.684 s |
| Incremental disabled | 2.821 s | 4.318 s | 2.805–8.438 s |
| lld threads=4 | 2.750 s | 3.411 s | 2.864–4.038 s |

Evidence: `target/hotpatch-proof/latency-explicit-replay-r2-fidelity/`.
The 16-CGU changed-crate setting is not a universal improvement. The 64-CGU
samples meet the warm target in this run; the shared-machine ordering and small
sample count do not establish a guaranteed ceiling. Confirm it against another
baseline before making it a default.

| Fidelity recording | Watch | Changed rustc | Host rustc | Compiler guard | dx link | Other driver | Receiver validation | Boundary + apply | Observed output | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 256 CGUs | 2 ms | 1,189 ms | 1,919 ms | 58 ms | 99 ms | 277 ms | 341 ms | 131 ms | 67 ms | 4,083 ms |
| 64 CGUs | 1 ms | 734 ms | 675 ms | 44 ms | 80 ms | 220 ms | 307 ms | 241 ms | 24 ms | 2,325 ms |

These are the actual median-total warm samples, with rounded columns. Their
changed-crate CPU times were 938 and 625 ms; host CPU times were 438 and 422 ms.
The larger change in host wall time therefore includes scheduling/IO variation,
not just different compiler effort. Publication includes Subsecond's library
loading and address relocation. One incremental-off sample spent 1.367 s in
publication under shared load; idle-boundary nanosecond measurements do not
describe that work. DLL loading stays at the boundary rather than moving
potential library initializers onto a concurrent render/simulation worker.

The watcher correction uses a short Windows read handle that shares reading and
atomic rename, while waiting asynchronously for an active writer to close.
This follows the documented [Rust share-mode contract](https://doc.rust-lang.org/std/os/windows/fs/trait.OpenOptionsExt.html#tymethod.share_mode)
and [Windows sharing rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).
It preserves legitimate empty/deleted-source rejection instead of skipping empty
files. The manual harness can hold a truncating writer open for 0–500 ms to
reproduce the notification race. No engine-frame lock or additional dependency
is introduced.

### Writer-close regression and confirmation

The delayed-write repro is deterministic: hold a truncating writer open for
150 ms before writing the complete 5,119-byte ECS source. With the snapshot
protection disabled, all four saves produced empty snapshots and transient
ARA7602 compiler rejections before recovering. With protection enabled, the
same four saves produced zero empty snapshots and zero rejections, with one
applied generation per save. PIDs 27132 and 26452 respectively retained the
entity, forward-moving state and determinism guards.

Evidence: `latency-writer-{off,on}-r1-sim` under `target/hotpatch-proof/`;
`source_snapshot` events record byte counts and duplicate decisions, never source
contents. Native mode also retains legitimate empty-file events so a real empty
source is rejected rather than silently ignored.

The final ordinary ECS run retained PID 15276 for sixteen edits/generations:

| Small ECS probe | Warm median | Warm range |
| --- | ---: | ---: |
| Existing incremental/256 CGUs | 2.149 s | 1.533–2.478 s |
| 1 CGU | 3.004 s | 1.464–28.630 s |
| 4 CGUs | 1.759 s | 1.504–3.466 s |
| 16 CGUs | 1.388 s | 1.382–1.421 s |

Evidence: `target/hotpatch-proof/latency-snapshot-r1-sim/`. The 28.630 s
sample used only 516 ms of rustc CPU, 4.344 s rustc wall time, 4.285 s inside
Ara's linker wrapper and another 19.36 s before dx delivered the patch. DX
classified 23.698 s as `Patch: Link`; PDB symbol matching afterward took about
10 ms. This reproduces a long tail outside large-crate compilation. Additional
dx link/cleanup/metadata/HTML timing boundaries distinguish the remaining tail;
do not attribute it to PDB parsing or Cargo locks without that evidence.

The render confirmation uses the same final watcher/compiler fixes. Disabling
deduplication, writer protection and the no-link correction yielded four saves
but five applied generations, warm median 4.031 s (3.909–6.393 s), PID 27792.
Enabling them yielded sixteen saves and sixteen generations in PID 24648.
Interleaving the unchanged 256-CGU and 64-CGU configurations twice gave:

| Fidelity recording, six warm samples each | Warm median | Warm range |
| --- | ---: | ---: |
| Default 256 CGUs | 2.476 s | 2.345–3.234 s |
| 64 CGUs | 2.766 s | 2.259–2.986 s |

Evidence: `latency-snapshot-{before,after}-r1-fidelity`. The earlier 64-CGU win
does not repeat against an interleaved baseline; keep the existing engine profile
instead of installing a blanket codegen-unit override. A small system/host leaf
can still benefit from fewer units. All source edits were restored and both
render confirmation hosts exited gracefully.

### macOS preview-4 ECS confirmation

The dedicated Mac checkout is based on main `792064a7` (which contains verified
Metal main `857a6aee`), with the owned timing/runtime/native-DX corrections from
this latency pass applied. It does not import unmerged lab or GPU changes.
AC was checked before each step; builds run one at a time with two Cargo jobs.

Four ECS system edits retained PID 8972 and entity 4294967293, with forward
component state and the replay/restore/network refusal checks passing. Default
incremental/256-CGU latency was 0.497 s first, then 0.494/0.493/0.494 s: warm
median **0.494 s**. DX's link phase was 34–35 ms; the system crate's rustc wall
time was 228–238 ms. Evidence is copied to
`target/hotpatch-proof/mac-p4/native-r1-sim/` on Windows. The manual controller
is in `mac-measure/`; it needs only the existing .NET 10 SDK and performs no
NuGet package downloads. Editor confirmation is still in progress.

The Windows editor lever run also exposed an 82.565 s tail: changed crate
14.573 s, host crate 53.707 s, links 4.676 s, and boundary publication 7.249 s.
The host compiler used only 1.156 s CPU. DX's finer phase profile assigns
53.743 s to compilation and 4.727 s to linking, rather than cleanup or PDB
matching. A nearby three-second PDH sample measured the D:/E: physical disk at
~181 ms per transfer with queue length 17, while memory remained ~8 GiB free
and multiple other Cargo/rustc processes were active. Evidence:
`target/hotpatch-research/windows-editor-{load,disk}-20261028.json` and
`latency-snapshot-after-r1-editor/`. This is evidence of a shared storage delay,
not grounds to hide the tail or weaken publication/compatibility checks.

### Windows editor lever comparison and final stage checkpoint

The unmodified editor console and the patched panel were captured through the
production editor protocol. PID 43232 and its session survived all 28 saves,
with exactly 28 applied generations. The restored host exited gracefully and
released its GPU queue. Captures were visually inspected.

| Editor configuration | Warm median | Warm range |
| --- | ---: | ---: |
| Before native corrections | 23.107 s | 17.977–35.386 s |
| Default incremental/256 CGUs | 15.205 s | 11.686–15.471 s |
| Changed editor crate, 16 CGUs | 15.604 s | 14.151–15.820 s |
| Small bin-only host, 16 CGUs | 12.062 s | 10.006–19.316 s |
| Editor and host, 16 CGUs | 12.048 s | 11.723–82.565 s |
| Editor, 64 CGUs | 34.439 s | 10.451–43.125 s |
| Editor incremental disabled | 18.216 s | 16.899–25.618 s |
| lld threads=4 | 12.932 s | 12.049–21.311 s |

Evidence: `latency-snapshot-{before,after}-r1-editor`. Shared-machine order and
the small sample count prevent promoting a blanket profile setting from this
table. Retain the existing profile. The editor remains above the latency target.

The following stage table uses a single representative warm edit per row, nearest
its configuration's median total. All columns are milliseconds and add to the
observed total, subject to rounding. Compilation includes the compatibility
facade; driver includes cache/symbol-table delivery and receiver validation.
The render-after representative is 2.545 s; its six-sample median is 2.476 s.

| Windows path | Watch | rustc + guard | dx links | Driver + validation | Boundary + apply | Output observation | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Render before | 1 | 2,173 | 1,355 | 454 | 14 | 33 | 4,031 |
| Render after | 2 | 1,428 | 529 | 527 | 34 | 25 | 2,545 |
| ECS before | 2 | 1,685 | 3,363 | 903 | 22 | 105 | 6,079 |
| ECS after, default | 1 | 1,409 | 114 | 466 | 134 | 25 | 2,149 |
| ECS after, probe 16 CGUs | 1 | 464 | 490 | 397 | 12 | 23 | 1,388 |
| Editor before | 1 | 13,725 | 7,898 | 1,092 | 212 | 178 | 23,107 |
| Editor after, default | 1 | 13,065 | 837 | 892 | 144 | 266 | 15,205 |

In that editor-after edit, the changed `ara-editor` crate took 7.200 s and the
bin-only host 5.785 s, plus 81 ms of facade work. Before the corrections, links
cost 6.668–14.389 s per warm editor edit; afterward default warm links cost
0.837–1.114 s. The old 34.568 s ECS result was a small-tip compiler tail, while
the old 33.060 s editor result included a queued stale build. They have different
causes; neither establishes that all reverse workspace consumers were rebuilt.

Recommend moving synchronous pass recording and panel drawing behind small leaf
crate boundaries, with stable handles/contexts supplied by their existing owners.
The small ECS leaf takes hundreds of milliseconds of compiler work; editor
compilation takes seconds even after removing redundant links. The measured
fidelity crate already reaches the target median, so prioritize a panel leaf and
the unusually costly editor host rather than splitting every engine crate.
No engine crate refactoring is included. Recheck shared storage contention before
using these wall times to set universal compiler defaults.
