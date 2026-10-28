# S11: Rust hotpatch checkpoints (2026-10-27)

**Windows functional checkpoint:** all three actual dispatch paths patch without
restarting. This remains an experimental dev feature: observed edits exceed the
2–3 s target and vary substantially on this shared PC with D: build outputs.

| Windows native CLI v2 | First body edit | Repeat body edit | Restore/recovery |
| --- | ---: | ---: | ---: |
| Fidelity render pass | 21.87 s | 4.33 s | 6.67 / 11.37 s |
| Bevy ECS system | 3.85 s | 34.57 s | 5.22 / 6.25 s |
| Editor panel | 20.59 s | 33.06 s | 29.47 / 13.18 s |

These are save-to-observed-output wall times, including workspace replay,
compilation, linking, delivery and output sampling. They are not DX's shorter
tip-only compile timer. Each row kept its process and state alive. Mac raster
rendering previously passed around 1 s; Mac ECS/editor measurements remain pending
AC power (the latest SSH battery check could not connect). No Mac build was run.

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

The lab adapter is still pending; its default remains rebuild plus warm restart.
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
