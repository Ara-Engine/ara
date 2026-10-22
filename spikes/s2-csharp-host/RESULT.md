# S2 result: C# host and reload gates

**Verdict: PASS on all five criteria. Keep D11 (C# is the scripting language).** The hosting design from R10 §D2 works as specified on Islam's machine, and both reload gates pass with margin. Four runtime findings change details of the M1 design; see [Findings](#findings-and-gotchas).

| Criterion | Target | Measured (3 rounds × 52 edits) | Result |
|---|---|---|---|
| L1 body-edit reload | p95 ≤ 100 ms | **p95 34.9–44.3 ms**, p50 18.7–21.7 ms (n=105) | PASS |
| L3 module swap, ≤5k-line module | p95 ≤ 450 ms | **p95 214–335 ms**, p50 178–199 ms (n=45, 5,198 lines) | PASS |
| C# chunk loop vs Rust, per entity, single-threaded | ≤ 2× | **1.64–1.82×** (`integrate`, quaternion math), **1.17–1.48×** (`gravity`) | PASS |
| State survives reload | checksum unchanged | **166/166** reload boundaries checksummed, 0 mismatches | PASS |
| Exceptions contained | host never crashes | **6/6** system exceptions caught inside parallel chunk calls; system disabled, re-enabled by next reload | PASS |

The two L1/L3 ranges are the two configurations measured: the default, and `--boost-ticks` (tick threads at above-normal priority; see finding 5). Both pass. R10's softer p50 targets also hold: L1 p50 ≤ 40 ms, and L3 p50 ≤ 200 ms (199 ms default, 178 ms boosted).

## Machine and versions

- AMD Ryzen 7 7800X3D (8C/16T), 31.1 GB RAM, RTX 4070 SUPER (unused), Windows 11 Pro build 26300, Windows Defender active.
- Rust 1.96.0, `bevy_ecs`/`bevy_tasks` 0.20.0, `netcorehost` 0.22.0.
- .NET SDK 10.0.202, runtime 10.0.6, Roslyn `Microsoft.CodeAnalysis.CSharp` 5.0.0, reference pack `Microsoft.NETCore.App.Ref` 10.0.6.
- Shared machine: other agents were running (CPU load around 30% outside the spike). Numbers are single-machine and noisy, so ranges are reported across runs.
- Under heavy contention (82–97% CPU from other agents' Rust builds), a final 1-round sanity run of the committed code still passed: L1 p95 61.8 ms, L3 p95 320 ms, 52/52 edits classified as expected, 0 state mismatches, 0 leaked ALCs. Its output was not saved.

## What was built

```
Rust host (src/)                                        C# (dotnet/Ara.Runtime, default ALC)
  schema.rs   repr(C) components, ABI structs, ──gen──▶  Generated/Abi.g.cs, Components.g.cs, Shapes.g.cs
              HostApi/ScriptApi tables, query shapes      Hosting/Exports.cs      [UnmanagedCallersOnly] entry points
  clr.rs      netcorehost → hostfxr → .NET 10              Hosting/SystemTable.cs  dispatch + fault containment
  sched.rs    bevy Schedule: one system per C# system,     Hosting/ModuleHost.cs   watcher, compile thread, L1/L3 staging,
              contiguous_par_iter_mut → run_system/chunk                           tick-boundary apply, ALC lifecycle
  bench.rs    criterion 3 + task-pool proof                Hosting/ModuleCompiler.cs  retained Roslyn compilation, emit, EnC
  harness.rs  fixed 60 Hz tick loop + editor thread        Hosting/EditClassifier.cs  conservative body-only syntax diff
  edits.rs    52 scripted edits                          game/ (gameplay module, 7 systems) + 100 generated filler files
```

1. **Rust host.**
   - `netcorehost` loads hostfxr and .NET 10.
   - `Ara.Runtime.dll` is loaded into the **default** ALC (`load_assembly_from_path`), so module ALCs share its types.
   - `Ara.Hosting.Exports.Init` is fetched as an `[UnmanagedCallersOnly]` pointer. It checks the ABI version, the layout hash and the struct sizes, then fills `ScriptApi`.
   - The `bevy_ecs` world holds 100k entities with `Transform` (32 B) and `Velocity` (24 B). Half also have `Health`, which gives two archetype tables.
2. **Generated ABI.** `schema.rs` declares everything that crosses the boundary through three macros:
   - `described_structs!` declares the components and ABI structs. Each one gets a descriptor holding names, `stringify!`'d types, `offset_of!` offsets, size, alignment and doc comments.
   - `fn_table!` declares `HostApi` and `ScriptApi`, the `#[repr(C)]` tables of `extern "C"` function pointers.
   - `shapes!` declares the query shapes.

   `ara-s2 gen` turns the descriptors into C#: explicit-layout structs with Rust's offsets, `delegate* unmanaged<…>` tables, and per-shape `ref struct` chunk views of `Span<T>`. It also writes the system delegates, `SystemRegistry.Add` overloads, the dispatch switch and `AbiInfo.LayoutHash`. No C# binding is hand-written.

   Two checks catch drift:
   - `gen --check` fails when the generated files are stale.
   - The runtime refuses to start against stale bindings. Verified: `stale generated bindings: host layout hash F05A…, C# AC63…`.
3. **C# systems over chunks.**
   - Each C# system becomes one Bevy system with the shape's query. `contiguous_par_iter_mut` (new in bevy 0.20) yields table slices.
   - Each slice becomes one `Chunk { len, entities*, col0..3 }`. That is one `run_system` UCO call per chunk; for this world, about 17 per frame per system.
   - User code is safe C#. It loops over `chunk.Transform` (`Span<Transform>`) directly in Rust column memory.
4. **L3.**
   - In-process Roslyn keeps a retained `CSharpCompilation` (`ReplaceSyntaxTree`). On change it emits a PE and PDB, loads them into a new collectible `AssemblyLoadContext`, calls `Register`, and pre-JITs the systems, all on the compile thread.
   - At the next tick boundary it swaps the system table and unloads the old ALC.
5. **L1.**
   - `DOTNET_MODIFIABLE_ASSEMBLIES=debug` is set before the runtime starts.
   - The classifier diffs syntax: bodies are stripped and the skeleton must be identical. It then emits `EmitDifference` against the retained `EmitBaseline`; EnC slot and lambda maps are read from the PDB.
   - `MetadataUpdater.ApplyUpdate` runs at the tick boundary. Anything else escalates to L3.
6. **Latency harness.**
   - A fixed-tick loop runs at 60 Hz. An "editor" thread saves 52 scripted edits per round, using `std::fs::write` on a copy of the module that a C# `FileSystemWatcher` watches.
   - Every edited code path calls `Probe.Stamp(N)` with a unique N. An edit completes on the first tick whose code stamped its value.
   - Edits without a stamp (constant changes, new files, registration order) complete on the tick after their reload report.
   - Latency is save → end of that tick, minus the checksum time at the boundary.
   - Rounds 2 and 3 first restore the pristine module (unmeasured).
7. **Fault containment.** Every UCO entry point catches everything. `SystemTable.Run` reports the first exception, marks the system faulted (it skips until the next reload) and returns a status code. Ticks continue.

## How to run

From `spikes/s2-csharp-host/` (Git Bash or PowerShell; set `CARGO_BUILD_JOBS=4` on a shared machine):

```sh
cargo build --release                                  # Rust host (target/release/ara-s2.exe)
./target/release/ara-s2 gen                            # regenerate C# bindings (checked in; `gen --check` verifies)
dotnet build dotnet/Ara.Runtime -c Release             # C# runtime + Roslyn
./target/release/ara-s2 bench                          # criterion 3 + task-pool proof (Release-IL module)
./target/release/ara-s2 bench --debug-il               # same with the hot (Debug-IL) module
./target/release/ara-s2 bench --no-enc                 # without DOTNET_MODIFIABLE_ASSEMBLIES
./target/release/ara-s2 reload --rounds 3              # the 156-edit latency harness (≈1 min per round)
./target/release/ara-s2 reload --rounds 3 --boost-ticks   # tick/task-pool threads at above-normal priority
./target/release/ara-s2 reload --rounds 1 --no-prewarm    # cold first-edit costs
```

Options:
- Reload flags: `--tick-hz N`, `--threads N` and `--verbose` (prints the C# per-phase logs).
- Experiment switches (environment variables, off by default):
  - `ARA_CONCURRENT_BUILD=0`: serial Roslyn emit.
  - `ARA_LOAD_FROM_FILE=1`: load each generation from a temp file instead of a stream.
  - `ARA_LOAD_NO_PDB=1`: load without the PDB.
  - `ARA_LOAD_PROBE=1`: prints load cost by context kind and image size during prewarm.

The raw outputs behind every number below are in [`results/`](results/).

## Measurements

### Reload latency (save → end of first tick running the new code, warm compiler)

| | Default (`results/reload-default.txt`) | `--boost-ticks` (`results/reload-boost.txt`) |
|---|---|---|
| L1, n=105 | p50 **21.7**, p95 **44.3**, max 102.2 ms | p50 **18.7**, p95 **34.9**, max 47.8 ms |
| L3, n=45 | p50 **198.8**, p95 **335.4**, max 377.1 ms | p50 **178.2**, p95 **214.1**, max 225.4 ms |
| Compile error reported (last good code keeps running), n=3 | p50 122.5 ms | p50 86.6 ms |
| Tick (`schedule.run`) p99 / max, whole session | 26.0 / 69.9 ms | 5.5 / 37.4 ms |
| Tick p99 inside reload windows | 43.3 ms | 23.4 ms |

**Phase medians, boosted run:**

| Phase | L1 (ms) | L3 (ms) |
|---|---|---|
| Watcher (save → event) | 0.0 | 0.0 |
| Settle + read + parse | 1.5 | 1.5 |
| Classify | 0.2 | 0.1 |
| Emit (delta / full image) | 3.6 (10 KB) | 91.2 (212 KB) |
| Stage: ALC load (incl. AMSI), `Register`, pre-JIT | 0 | 70.8 |
| Wait for tick boundary | 10.7 | 9.5 |
| First tick with new code | 3.8 | 4.1 |

**Cold-start costs**, reported separately:

| Step | Cost |
|---|---|
| hostfxr + runtime init + first function pointer | 35–50 ms warm disk (up to 171 ms on the first run) |
| `Init` (layout check, table fill) | 1.7–2.8 ms |
| First compile + load of the 5.2k-line module (cold Roslyn, mostly JIT) | 1.66–2.0 s |
| Prewarm, run once at start | 395–540 ms total |
| ↳ classify | 28–31 ms |
| ↳ first `EmitDifference` | 144–185 ms (2.4–3.4 ms warm) |
| ↳ second full emit | 139–236 ms |
| ↳ ALC load | 66–84 ms |
| First tick (JIT + task-pool spin-up) | 5–6 ms |
| First L1 / first L3 **without** prewarm (`results/reload-no-prewarm.txt`) | 202 ms / 206 ms |
| First L1 / first L3 **with** prewarm | 35 ms / 200–229 ms |

### C# chunk loop vs Rust (`results/bench-*.txt`, 100k entities, median of 7×200 frames)

| Loop (ns/entity) | C# Release IL | Rust | C#/Rust |
|---|---|---|---|
| `integrate`: position += v·dt, quaternion spin + normalize | 6.1–7.3 | 3.5–4.4 | **1.64–1.82×** |
| `gravity`: velocity update + ground bounce | 0.72–1.01 | 0.53–0.86 | **1.17–1.48×** |
| Same two loops, with bevy change ticks marked | 6.0–7.5 / 0.96–1.06 | 3.6–4.2 / 0.75–0.98 | 1.68–1.78× / 0.98–1.37× |
| `integrate` on the task pool (8 threads, wall/entity) | 1.46–1.86 | 1.03–1.27 | 1.33–1.73× |
| Hot module (Debug IL) `integrate` / `gravity` | 34.8–36.3 / 4.7–6.4 | — | **9.7–9.8× / 7.4–7.6×** |
| `run_system` on an empty chunk (UCO + dispatch + probe call back) | 15–18 ns/call | — | — |

- Both sides are measured with the same contiguous query and the same chunk walk. The Rust port mirrors the C# operations, including division-based normalization.
- Release perf is identical with and without `DOTNET_MODIFIABLE_ASSEMBLIES=debug` (`bench-no-enc.txt`).
- **Task-pool proof:** `integrate` ran on all 8 `TaskPool (0..7)` OS threads, about 5,900 chunk calls each. The CLR saw 9 distinct threads call `run_system` (8 pool threads plus main). Native threads attach to the CLR automatically on their first call.

### State, faults, unload

| Area | Result |
|---|---|
| State | Every reload boundary checksummed every component column before and after `tick_boundary`: 166 boundaries, 0 mismatches. The schedule was rebuilt 14 times, when L1 or L3 changed the system list. |
| Faults | An off-by-one `IndexOutOfRangeException` in `drag` and a `NullReferenceException` in `regen`, each 3 times, all thrown inside parallel chunk calls. Each was reported once (`drag: IndexOutOfRangeException … (disabled until next reload)`), and the system stopped running. The fix edit's stamp was observed, which proves the next reload re-enabled it. The host never stopped ticking. |
| Classification | 156/156 edits produced the expected layer. L3 escalations: structural 39, new file 3, closure 3. |
| ALCs | 61 created per 3-round session. 0 alive after the shutdown GC: unload works, once finding 2's setting is applied. |

## Findings and gotchas

1. **A missed rude edit crashes the process. The runtime does not catch it. (Severe.)**
   - My first classifier allowed body edits to methods that contain lambdas. Changing `static h => h.Value < 10f` to `< 20f` produced a delta that Roslyn emitted without a syntax map. `ApplyUpdate` accepted it, and the next call died with an **uncatchable `AccessViolationException`**.
   - R10 warned that the runtime would not reject unsafe deltas. This is that, seen for real.
   - The classifier now escalates any changed body that contains a lambda, anonymous method, local function or query (`Escalation.Closure`).
   - For M1: L1 for closures needs a proper syntax map. Roslyn's own EnC analyzer is internal. Until we have one, closures stay L3, and the classifier must be fuzzed with recorded edits (R10 already listed this).
2. **Tier-0 JIT of Release-IL loop methods pins collectible ALCs forever.**
   - With default tiering, an optimized module whose systems ran never unloaded (1/1 leaked, even after 2.5 s of forced GCs).
   - Bisected with env vars:
     - `TieredCompilation=0` fixes it.
     - `TC_QuickJitForLoops=0` fixes it.
     - `TC_OnStackReplacement=0`, `TC_CallCounting=0`, `TieredPGO=0` and disabling guarded devirtualization do not.
   - Debug-IL (hot) modules are unaffected.
   - Fix used: `<TieredCompilationQuickJitForLoops>false</TieredCompilationQuickJitForLoops>`. Loop methods go straight to optimized code. The cost is no dynamic PGO for loop methods; the bench showed no measurable loss.
   - This looks like a runtime bug worth a minimal repro and an upstream issue.
3. **Release builds disable hot reload by default.** The SDK writes `System.Reflection.Metadata.MetadataUpdater.IsSupported: false` into a Release `runtimeconfig.json`, so a Release-built host must set `<MetadataUpdaterSupport>true</MetadataUpdaterSupport>`.
4. **On Windows, AMSI costs about 65 ms per L3: roughly 40% of the swap.**
   - `LoadFromStream` takes 62–84 ms on the first load of new bytes, and under 1 ms for bytes already seen (the probe reloaded the same image five times).
   - No GC runs during the load. The cost scales with size: 13 ms first load for a 3 KB image.
   - `coreclr.dll` imports `AmsiScanBuffer`, and there is no runtime knob to skip it.
   - Loading from a temp file is worse (80–120 ms), because of Defender's on-access scan.
   - I infer this is the Defender/AMSI scan of in-memory assemblies; I did not confirm it with ETW.
   - Options:
     - A Defender process exclusion for the editor. This is the user's choice; document it.
     - Smaller modules.
     - Accept it: L3 still passes.
   - macOS and Linux don't pay it.
5. **In-process Roslyn makes the game hitch while you edit.**
   - Default run: 132 of 3,600 ticks took over 8 ms, with tick max 70 ms, and GC paused for 2.2 s in total (209 collections).
   - Two causes:
     - **CPU contention.** Roslyn's parallel emit starves the task pool. A serial emit (`ARA_CONCURRENT_BUILD=0`, `results/reload-serial-emit.txt`) cut the reload-window tick p99 from 36.5 ms (`results/reload-gc-attribution.txt`) to 5.4 ms, but pushed L3 p95 to 530 ms (FAIL), so that is not the fix. Raising the tick and task-pool threads to above-normal priority (`--boost-ticks`) fixed it with no latency cost.
     - **GC pauses.** Ephemeral GCs over Roslyn's 50–75 MB heap average about 9–15 ms. With boosting, 72% of the remaining slow-tick time is GC. A 256 MB gen0 budget (`DOTNET_GCgen0size=0x10000000`, `results/reload-boost-gen0-256mb.txt`) halved the GC count and left 2 slow ticks in 1,149, but it only shifts the problem.
   - The robust fix is an out-of-process compile server. This is how `dotnet watch` works: Roslyn runs in its own process and the app only applies deltas. See the recommendation.
   - The three single-round experiments above (serial emit, GC attribution, gen0 budget) ran one round each, so treat them as directional.
6. **Bevy 0.20's contiguous queries are the chunk ABI, for free.**
   - `contiguous_iter_mut` and `contiguous_par_iter_mut` yield `(&[Entity], ContiguousMut<T>, &[T])` per table batch.
   - `ContiguousMut::split` marks change ticks per batch, at about 0.0–0.3 ns/entity.
   - `Entity` is `repr(C, align(8))` and equal to its `to_bits()`, so entity spans cross as `u64`.
   - Bevy tables are not fixed-size chunks. The parallel iterator splits 2 tables into about 17 batches per frame.
7. **Waiting for the tick boundary is the largest single L1 cost**: about half a tick (8–11 ms median at 60 Hz). That is inherent to applying updates between ticks. An editor ticking at 120 Hz, or applying at a frame boundary, halves it.
8. **Windows timed waits round up to the 15.6 ms timer tick.**
   - The 1 ms watcher "settle" (which coalesces truncate+write events) must spin with `Thread.Yield`, not use a timed `TryTake`.
   - Rust's `thread::sleep` uses high-resolution timers and is fine.
   - Reading a just-saved file occasionally takes about 5 ms. My guess is Defender scanning on close; unverified.
9. **The hot-module penalty on these loops is 7.4–10×.** This matches R10's 4–24×, and it confirms that module heat must be visible in the profiler.
10. **Register is reloadable.**
    - An L1 edit to `ModuleEntry.Register` (register, reorder) re-runs registration after `ApplyUpdate`.
    - A changed system list sets `BOUNDARY_SYSTEMS_CHANGED`, and Rust rebuilds its schedule (14 times per session).
    - After `ApplyUpdate`, delegates and function pointers obtained earlier call the new body, so L1 never needs a rebind.
11. **Constants fold into call sites**, so changing a `const` must be L3. The skeleton compare treats initializers as structure, so const changes escalate.
12. **A fault mid-chunk leaves a torn tick.** Rows before the throw are written. This is fine for the editor. Netcode/rollback (R09) must decide whether a faulted tick is rolled back.
13. **Some failures cannot be contained in-process:** access violations (finding 1), stack overflow and `FailFast` kill the process. Those are L4 (snapshot restart), as R10 planned.
14. **ALC loading must use the default context.**
    - netcorehost's `get_delegate_loader_for_assembly` loads the component into an isolated context.
    - Module ALCs would then bind a second copy of `Ara.Runtime`, and its types would not match.
    - `load_assembly_from_path` (the default ALC) plus a module ALC whose `Load` returns the host's `Ara.Runtime` avoids that.

## Recommendation

**Keep D11 and build M1's `C# host v1` on this design.** Make these changes in the build order:

1. **Move Roslyn into a warm compile-server process.** It owns the watcher, the retained compilations, the classifier and the EnC baselines, and ships images and deltas to the game or editor process over a pipe.
   - Measured in-process: emit is 3.6 ms (L1) and 91 ms (L3). A pipe hop for 10–212 KB is well under a millisecond. Latency should stay the same while the 10–15 ms Roslyn GC pauses leave the game process.
   - It also isolates compiler crashes, and one compiler can serve both the editor and the play-mode child (S5).
   - Not prototyped in S2; it is the first thing M1 should measure.
2. **Raise tick and task-pool threads to above-normal priority** while a compile is in flight. Measured: reload-window tick p99 43 → 23 ms, with no latency cost.
3. **Runtime config:**
   - `TieredCompilationQuickJitForLoops=false` (finding 2), until an upstream fix lands.
   - `MetadataUpdaterSupport=true`.
   - `GCLatencyMode.SustainedLowLatency` during play.
4. **Classifier policy:** body-only, with **no closures, constructors, initializers or state machines** at L1. Fuzz it, and treat any new L1 allowance as a reviewed change, because a mistake is a process crash (finding 1).
5. **Keep the "Rust never holds pointers into module code" rule.** Systems are ids into a C# table that is swapped at tick boundaries. That rule is what makes unload safe and L3 cheap.

## Proposed ABI shape and generator for M1 `C# host v1`

- **One entry point:** `ara_script_init(const HostInit*, ScriptApi*) -> i32`.
  - It is `[UnmanagedCallersOnly]`, resolved through hostfxr in development, and gets `EntryPoint = "ara_script_init"` for NativeAOT.
  - `HostInit = { abi_version, layout_hash, ctx, HostApi }`, with the table passed by value.
- **Two `#[repr(C)]` function-pointer tables**, declared once in Rust. As built in S2, plus the M1 additions:

  | Table | Functions |
  |---|---|
  | `HostApi` (Rust → C#) | `log`, `now_ns` (diagnostics only), `report_reload`, `report_fault`, **plus** `cmd_push(buf, bytes, len)` for structural changes at sync points, `resource_ptr(id)`, `register_schema(json)` for C#-declared components |
  | `ScriptApi` (C# → Rust) | `load_module`, `prewarm`, `has_pending`, `tick_boundary`, `system_count`, `system_info`, `run_system(id, *const Chunk, *const Tick) -> status`, `diag`, `shutdown` |

  Every C# entry point catches everything and returns a status. Trivial host calls (`now_ns`, `stamp`) are candidates for `SuppressGCTransition` after measurement.
- **Chunks:**
  - `Chunk { len, column_count, entities: *const u64, cols[N] }`, with one `run_system` call per contiguous batch. The 15–18 ns boundary cost is noise at chunk granularity.
  - S2 used fixed shapes (`shapes!`). M1 should build queries from the system's declared component ids and access at registration. Write a custom `SystemParam` that walks the matched tables and takes column pointers under the declared access, or use contiguous iteration if bevy's dynamic `FilteredEntityMut` supports it; that needs checking.
  - Written columns mark change ticks per batch.
- **Layout safety:**
  - `layout_hash` covers every descriptor: names, types, sizes, alignment, offsets, table signatures and shapes.
  - C# also runs `VerifySizes()`. A mismatch refuses to start with an actionable message.
  - `gen --check` becomes a local gate (D9).
- **Generator, in two halves:**
  - **Rust → C#.** Today it is macro descriptors plus `ara-s2 gen`. In M1 it becomes the schema DB (A2) feeding `ara gen csharp`, which writes `Ara.Generated` with:
    - components with explicit `FieldOffset`s taken from `offset_of!`;
    - ABI tables;
    - chunk views (`ref struct` of `Span<T>`/`ReadOnlySpan<T>` by declared access);
    - system delegates and registry overloads;
    - the dispatch switch;
    - XML docs from the Rust doc comments, which become inspector tooltips and agent docs.

    It is an explicit command, not a `build.rs`, so its output is reviewable and checked in.
  - **C# → registration.** A Roslyn incremental source generator turns `[System]`-attributed methods into `Register(SystemRegistry)`, plus their read and write sets. It replaces the hand-written `ModuleEntry`; no reflection is needed beyond locating `Register`, which keeps it AOT-safe. The same generator emits descriptors for C#-declared components back to Rust.

## Module and assembly layout

| Assembly | Load context | Rebuilt when | Contents |
|---|---|---|---|
| `Ara.Runtime` | Default ALC, never unloaded | Engine rebuild (L4) | Exports, system table, fault containment, module host (the compile client, once item 1 lands), and the public API |
| `Ara.Generated` | Default ALC (can merge into `Ara.Runtime`) | Rust schema change (L4) | Everything `ara gen csharp` writes |
| Gameplay module, one per folder (`ara.module.toml`) | One collectible ALC per module, per generation | L1 (EnC) when hot; L3 swap otherwise | Systems and behaviours; no mutable statics; no NuGet without a leak test |

Module rules:
- **Size:** soft cap about 5k lines. At 5.2k lines, a Debug emit takes about 100 ms and a load about 65 ms, which AMSI dominates (finding 4). Emit cost scales with the assembly, not the edit (R10).
- **References:** framework ref pack + `Ara.Runtime` + upstream modules. A structural change to a module rebuilds its dependents in parallel.
- **Heat:** hot modules are Debug IL with EnC on (7–10× slower loops); cold modules are Release IL. The first edit to a cold module makes it hot with one L3.
- **Shipping:** one NativeAOT image (S7); no ALCs or Roslyn.

## Proposed decision

> **D11 (confirmed by S2, 2026-10-23).** C# 14 / .NET 10 remains the single scripting language. S2 passed every gate on Islam's machine: L1 p95 35–44 ms, L3 p95 214–335 ms for a 5.2k-line module, C# chunk loops at 1.2–1.8× Rust, ECS state unchanged across 166 reloads, and 6/6 system exceptions contained.
>
> **A13 (new). C# hosting and reload.**
> - Rust hosts .NET through hostfxr in development, with `Ara.Runtime` in the default ALC. One generated `#[repr(C)]` ABI exists: `HostInit` + `HostApi` + `ScriptApi`, layout-hashed, with C# bindings generated from Rust descriptors.
> - Every entry point is `[UnmanagedCallersOnly]` and catches everything.
> - C# systems run as one call per contiguous ECS batch, from Bevy task-pool threads, and are addressed by id into a C# table that is swapped only at tick boundaries. Rust never holds pointers into module code.
> - Reload is L1 (EnC, body-only edits without closures, constructors, initializers or state machines, in hot Debug-IL modules) or L3 (a new collectible ALC per module generation).
> - Runtime config: `QuickJitForLoops=false`, `MetadataUpdaterSupport=true`.
> - Roslyn moves to a warm out-of-process compile server in M1. Tick threads run at above-normal priority while it compiles.
