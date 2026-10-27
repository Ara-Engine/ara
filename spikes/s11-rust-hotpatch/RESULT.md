# S11: Rust hotpatch, first checkpoint (2026-10-27)

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

Tested subsecond and dx 0.7.10. Latest stable dx is 0.7.10; 0.8.0-alpha.3 also
exists but is not the tested tool/protocol. Source reads included published
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
