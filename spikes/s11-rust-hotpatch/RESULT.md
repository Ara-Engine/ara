# S11: Rust hotpatch checkpoints (2026-10-27)

**Current: real native render patches work on both platforms.** Windows body
edits measured 3.86–8.94 s; the first Mac body edit measured 1.04 s. A repeat-save
CLI cache bug still blocks readiness; simulation/editor live proofs are pending.

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
be silently ignored. This blocks calling the integration solid. A small optional
CLI patch is being verified: native-engine mode replays Rust saves directly,
including queued saves, bypassing RSX diffing. Normal Ara builds still need no dx.

Mac evidence: `target/hotpatch-research/mac-render-r1{.log,-measurements.json}`,
before/after PNGs there, and the isolated remote worktree's
`target/hotpatch-proof/mac-render-r1/`. Both probe and original driver were stopped.
Simulation/editor live-patch measurements remain pending on both platforms.

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
