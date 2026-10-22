# R10: C# as the gameplay language on the Rust core, with near-instant iteration

*Research track R10 · 2026-10-23. Notation: [S#] = web sources, [C#] = code read in `.refs/` or in a read-only temp clone of s&box (`Facepunch/sbox-public` @ `f7bedf4`, 2026-10-22), [M#] = measurements I ran for this report. Measurement setup: AMD Ryzen 7 7800X3D (8C/16T), 31 GB RAM, Windows 11, .NET SDK 10.0.202 / runtime 10.0.6, Roslyn `Microsoft.CodeAnalysis.CSharp` 5.0.0, Rust 1.96, `netcorehost` 0.22.0. The scratch code lives in `%TEMP%\r10\bench` and is not part of the project. These are single-machine micro-benchmarks (medians of 5–10 runs), not product numbers. M1 = in-process Roslyn compile and ALC cycle. M2 = EnC deltas driven by a custom host. M3 = Rust-hosted CLR boundary, batching and GC. M4 = Debug-IL versus optimized execution. "(unverified)" marks claims I could not confirm. Nothing from `.refs/` was built or run.*

---

## TL;DR

- **Recommendation: C# (C# 14 on .NET 10 LTS) as snengine's one high-level language**, covering gameplay, UI logic, editor extensions and agent scripts (`sn_run`). Drop TypeScript and V8 entirely. Four things settle it:
  - The owner's move to a native, non-web UI removes TypeScript's biggest synergy.
  - iOS is a first-tier target. Jitless V8 runs 40–80% slower there [S27], while NativeAOT on iOS has been a *supported* .NET target since .NET 9 [S12,S13].
  - C# value types map 1:1 onto Rust ECS columns.
  - s&box's MIT-licensed C# toolchain gives us reusable prior art.
- **TypeScript still wins three things:** in-process sandboxing, web export and corpus size. The design below contains each of these rather than ignoring it.
- **Sub-100 ms body-edit reload is real and measured.** A custom host can drive .NET's own Hot Reload machinery. Roslyn `EmitDifference` produced a method-body delta in **4–25 ms**, and `MetadataUpdater.ApplyUpdate` applied it in **0.1–3 ms**, against an 8k-line assembly. This worked even **inside a collectible AssemblyLoadContext (ALC)** [M2].
- **The catch with Edit-and-Continue (EnC):** it requires Debug IL. The JIT runs Debug IL **4.4× slower on a scalar loop and 24× slower on `Vector3` math** [M4]. So only "hot" assemblies (the ones being edited) are EnC-enabled, and heavy systems stay optimized or move to Rust.
- **Structural reload cost scales with assembly size, not edit size.** Roslyn's "incremental" compile reuses only parse and declaration work; emit is still whole-assembly. Measured: **42 ms at 800 lines, 389 ms at 8k, 614 ms at 24k** [M1]. Small modules (≤~5k lines) are a latency requirement.
- **Layered reload:**

  | Layer | What it handles | Target / cost |
  |---|---|---|
  | L0 | Data and UI edits, no compile | ≤30 ms |
  | L1 | EnC body edits | p50 ≤40 ms, p95 ≤100 ms |
  | L3 | Module swap into a fresh collectible ALC | p50 ≤200 ms, p95 ≤450 ms |
  | L4 | State-preserving process restart | 1–5 s |

  Gameplay state lives in Rust ECS columns, so L3 is a rebind plus column migration. It is not s&box's reflective object-graph walk.
- **s&box confirms the approach and lends us code.** It runs in-process incremental Roslyn and a syntax-diff "only method bodies changed" classifier. That fast path is implemented with MonoMod detours; everything else takes a reflective instance-upgrade path. It also has a two-layer API whitelist and a Razor-based native UI [S7,C7]. Its advertised "milliseconds" figure is the fast path only.
- **The Rust↔CLR boundary is cheap if calls are batched** [M3]:

  | Measurement | Result |
  |---|---|
  | Runtime init | 93 ms |
  | Rust→C# call | 3.7 ns |
  | C#→Rust call | 3.3 ns (1.1 ns with `SuppressGCTransition`) |
  | C# system over Rust-owned component memory | 1.74 ns/entity, vs 1.09 for the same loop in Rust |

  So the ABI does one call per archetype chunk, never one per entity.
- **Platform plan.** CoreCLR JIT runs the editor and desktop dev builds. NativeAOT ships Windows, Linux, macOS and iOS; Godot ships its iOS C# export the same way [C5]. Android NativeAOT is still labelled "experimental" [S12], so prove it early in the build order. Web is a C# gap.
- **Sandboxing: there is no in-process hard sandbox in modern .NET, so code runs in trust tiers.**
  - Project code is trusted and checked by analyzers.
  - `sn_run` scripts run in a **warm out-of-process C# worker**. Their only write path is typed Engine Protocol commands, so every write lands in the op log.
  - User-generated content (UGC) follows the s&box model: compile from source, IL whitelist, process isolation on servers.
- **Agent stance: use Unity priors only where they're safe.** Reuse Unity's concepts, and Unity's exact names only where behaviour is identical (`[Range]`, `[Tooltip]`, `[FormerlySerializedAs]`). Rename everything that behaves differently, so a wrong prior fails to compile. An analyzer then offers a machine-applicable fix.

---

## Findings

### 1. Iteration: what .NET 10 gives a custom host

**Measured [M1, M2]** (warm unless stated; Debug IL, embedded PDB):

| Operation | ~800 lines (20 files) | ~8k lines (200) | ~24k lines (600) |
|---|---|---|---|
| Full `CSharpCompilation.Create` + `Emit` | 49 ms (cold first run 1,292 ms) | 517 ms | 763 ms |
| Retained compilation: parse 1 file + `ReplaceSyntaxTree` + `Emit` | 42 ms | 389 ms | 614 ms |
| Same edit, `GetDiagnostics()` only (agent typecheck loop) | 22 ms | 169 ms | 241 ms |
| Collectible ALC: load + first call + `Unload` + collected | 44 ms | 27 ms | 47 ms |
| EnC delta: `EmitDifference`, one member changed, 200-file baseline | — | 4–25 ms (first 102 ms) | — |
| `MetadataUpdater.ApplyUpdate` | — | 0.1–2.6 ms | — |

What this establishes:

1. **The Hot Reload runtime path is open to custom hosts, not just `dotnet watch` and Visual Studio.** The host must set `DOTNET_MODIFIABLE_ASSEMBLIES=debug` before the runtime starts. CoreCLR 10.0.6 then reports these capabilities: `Baseline AddMethodToExistingType AddStaticFieldToExistingType AddInstanceFieldToExistingType NewTypeDefinition ChangeCustomAttributes UpdateParameters GenericUpdateMethod GenericAddMethodToExistingType GenericAddFieldToExistingType AddFieldRva` [M2].
   - I applied three kinds of edit: a body change, an added method, and an added instance field on a class. Existing instances read the new field's default value.
   - All three worked in both a normal and a **collectible** ALC [M2].
   - Roslyn's supported-edit list also covers lambdas, async methods, iterators and (since .NET 8) generics [S1,S2].
   - Excluded edits: interface changes, new virtual/abstract/override members, base-type changes, type deletion, and edits around active statements [S1,S2].
   - `ApplyUpdate` only affects future calls. Frames already on the stack keep running the old IL [S3].
2. **A host cannot apply updates while a debugger is attached.** CoreCLR's `ApplyUpdate` throws `NotSupported_DebuggerAttached` in that case (`src/coreclr/vm/assemblynative.cpp`) [S4]. While an IDE debugger is attached, deltas must go through the debugger, or the host falls back to a full assembly reload. Visual Studio also lists "debugging an embedded runtime application" as unsupported for its own EnC [S2].
3. **Roslyn's rude-edit analyzer is not a public API.** `dotnet watch` uses `HotReloadService`, which is `internal sealed` in `Microsoft.CodeAnalysis.ExternalAccess.HotReload` [S6]. Rude-edit kinds include `InsertOrMoveStructMember` and `InsertVirtual` [S5].
   - My harness emitted and applied an *added struct field*, which Roslyn's analyzer classes as a rude edit. The runtime raised no error [M2].
   - **The runtime will not reliably reject unsafe deltas, so a custom host needs its own conservative edit classifier.** s&box does exactly this. `ILHotloadProcessor` enables its fast path only when the set of files is unchanged and only statement blocks changed (`engine/Sandbox.Generator/Processors/ILHotload.cs`) [C7].
4. **EnC forces unoptimized code, and the penalty is large.** Measured with Debug versus Release IL, both on Tier-1 JIT [M4]:

   | Loop | Debug IL | Release IL | Slowdown |
   |---|---|---|---|
   | Scalar integrate | 7.11 ns/entity | 1.63 ns/entity | 4.4× |
   | `System.Numerics.Vector3` integrate (operators not inlined) | 44.6 ns/entity | 1.8 ns/entity | 24× |

   s&box users pay this too. Its local-development builds default to Debug, and only published games compile as Release (`engine/Sandbox.Compiling/CompilerConfiguration.cs:46-50`) [C7].
5. **Roslyn has no incremental emit.** A retained compilation saves only about 25% (517 → 389 ms at 8k lines). Emit time follows assembly size, so assembly granularity sets the floor for full reloads.
   - A cold Roslyn costs about 1.3 s, almost all of it JIT, so prewarm it at editor start.
   - The first `EmitDifference` costs about 100 ms, so prewarm that path too.
6. **ALC unload is cheap when nothing pins the context, and painful when something does.**
   - Before calling `Unload()`, Flax runs GC twice and frees cached method, field and type handles. It also clears its JSON serializer cache. It even clears `System.ComponentModel.TypeDescriptor`'s private tables by reflection, as a workaround for dotnet/runtime#30656 (`.refs/flax/Source/Engine/Engine/NativeInterop.Unmanaged.cs:1021-1125`) [C2].
   - Godot disables collectible ALCs in the game player because their limitations "cause issues with mocking libraries". It reloads only in the editor (`.refs/godot/modules/mono/csharp_script.cpp:625-640`) [C4].

**How the four reference stacks reload:**

| Engine | Compile | Fast path | Full path | Where state lives |
|---|---|---|---|---|
| **s&box** (MIT, `net10.0`) | In-process Roslyn 5.0. Keeps a retained `CSharpCompilation` and calls `ReplaceSyntaxTrees`. Runs custom generators, a whitelist walk, then emits to memory with an embedded PDB (`Compiler.Build.cs:165-290`) | Body-only edits are patched with **MonoMod.Core detours** from old methods to newly compiled ones. Editor only (`PackageLoader.TryFastHotload`, `ILHotload.cs`) | Loads the new assembly, then walks statics and watched roots with reflection/Cecil. It **upgrades every instance** of each changed type through `IHotloadManaged.Created(state)` (`Sandbox.Hotload/UpdateReferences.cs`) | Managed GameObjects and Components |
| **Flax** 1.13 | Flax.Build, out of process | — | Serialize scenes → destroy game objects → unload the collectible ALC → load → deserialize (`Scripting.cpp:732-803`) [C1,S22] | Managed + native objects |
| **Godot** 4.8 | `dotnet build` (MSBuild) | — | Editor only: serialize script-instance properties and delegates, unload, reload. In a running game, only IDE Hot Reload helps, and its limits and failures are documented [S20,S21] | Native objects + exported properties |
| **Unity** 6.6 → 7.0 | Unity's compilation pipeline | — | 6.6 makes Fast Enter Play Mode the default, so statics are no longer reset. Domain reload "goes away entirely" in 7.0, replaced by ALC-based granular reload. Unity says the big speed-ups may land only after 7.0 [S10,S11] | Managed objects + native serialization |

s&box's advertised "within a few milliseconds" [S8] is accurate for body edits. Every other edit pays for reflective instance migration. snengine avoids that cost by keeping authoritative state out of the managed heap (R09's rule). Its full path is therefore only a **rebind of system and behaviour tables plus a migration of changed component columns**.

### 2. Hosting .NET from Rust

**Measured [M3]** (`netcorehost` 0.22.0 [S24] → hostfxr → .NET 10.0.6):

| Measurement | Result |
|---|---|
| `load_hostfxr` + `initialize_for_runtime_config` + first function pointer | 92.8 ms |
| Rust → C# `[UnmanagedCallersOnly]` call (`fn(i32,i32)->i32`) | 3.6–3.8 ns/call |
| C# → Rust through `delegate* unmanaged<int,int>` | 3.29 ns/call |
| Same with `delegate* unmanaged[SuppressGCTransition]` | 1.10 ns/call |
| C# system over a `Span<Transform>` of a Rust-owned `Vec`, 100k entities, one call per frame | 1.74 ns/entity (same loop in Rust: 1.09) |
| NativeAOT `NativeLib=Shared` build of the same library (win-x64) | 1.03 MB DLL |

Patterns that follow:

- **One ABI for JIT and AOT builds.** In development, Rust fetches `sn_script_init` through `get_function_with_unmanaged_callers_only` [S24,S25]. In NativeAOT builds, the same method carries `[UnmanagedCallersOnly(EntryPoint = "sn_script_init")]`. It is linked statically on iOS and loaded as a shared library elsewhere.
- **Godot already does this.** It uses hostfxr in the editor and on desktop (`.refs/godot/modules/mono/mono_gd/gd_mono.cpp`) and NativeAOT on iOS [C5,C6]. Its interop also uses generated tables in both directions. C# calls native code through `delegate* unmanaged` pointers in `NativeFuncs.cs`. Native code calls managed code through a `ManagedCallbacks` struct (`mono_gd/gd_mono_cache.h:71`) [C6].
- **Exceptions must never cross the boundary.** An exception that escapes an `[UnmanagedCallersOnly]` method kills the process. Generated wrappers therefore catch everything, report it, return a status code and disable that system until the next reload.
- **Threads:** a native thread attaches to the CLR automatically on its first native-to-managed call. C# systems can therefore run on Bevy's task pool, provided the generator exports their read/write sets to the scheduler.
- **Debugging.**
  - Visual Studio ("Managed (.NET Core)", or mixed-mode on Windows), Rider and VS Code (`coreclr` attach) can all attach to any process with CoreCLR loaded. Godot and Flax are debugged this way. This is standard practice, but I did not re-verify it on our host.
  - Ship generated launch profiles that start `snengine.exe` directly.
  - The embedded PDBs from in-memory Roslyn emits make breakpoints work in reloaded code.
  - Finding 1.2 still applies: while a debugger is attached, every reload uses the full L3 path.

### 3. Shipping on five platforms

| Target | .NET status (Oct 2026) | Precedent |
|---|---|---|
| Windows / Linux / macOS | NativeAOT supported on x64 and arm64, plus x86 on Windows and arm on Linux [S12] | s&box and Flax use CoreCLR JIT on desktop |
| iOS | NativeAOT **supported since .NET 9**, including non-MAUI projects via `PublishAot` + `PublishAotUsingRuntimePack` [S12,S13]. .NET 11 MAUI moves iOS to CoreCLR: composite ReadyToRun, plus an always-on interpreter for methods that weren't precompiled [S14,S15]. .NET 11 RC2 for Apple targets requires Xcode 27 [S16] | **Godot's** `Sdk/iOS.props` sets exactly those two properties [C5]. **Flax** uses Mono AOT (`MonoAOTDynamic`) on iOS [C3] |
| Android | NativeAOT is **"Experimental, no built-in Java interop"** [S12]. In .NET 11, MAUI defaults to CoreCLR on Android, with NativeAOT as an opt-in experiment [S14,S31]. Uno ships NativeAOT on Android and iOS; it reports Android startup falling from 0.895 s to 0.348 s (secondary source) [S18] | **Godot** forces `UseMonoRuntime` on Android unless `PublishAot` is set [C5]. Godot's docs still label Android and iOS C# support "experimental" [S19] |
| Web (bonus tier) | No NativeAOT target. Blazor WebAssembly stays on Mono [S14]. Godot 4 C# projects still can't export to the web [S19] | **Gap** |

**Reconciling R08.** R08 calls iOS NativeAOT "experimental". That label appears only in the .NET 8 tab of Microsoft's platform table; the .NET 9+ tab lists iOS as supported [S12]. The MAUI NativeAOT guide (updated 2026-10-01) carries no experimental label either [S32]. It does document real limits:
- no managed debugger on a published device build (lldb only);
- no heap analysis;
- no EventPipe over TCP.

So device-side *development* builds should prefer a JIT or interpreter runtime where one exists, and NativeAOT remains the shipping model. R08's broader worry, that C# is second-class on mobile and web, holds for Android (experimental) and web (a gap), not for iOS.

**NativeAOT constraints** are the same family as Unity's IL2CPP restrictions [S26]:

- No `Assembly.Load*`, so an AOT build cannot load mods at runtime.
- No `Reflection.Emit`.
- `Expression.Compile` runs interpreted.
- Reflection is a trimming hazard.
- Every generic instantiation over a struct gets its own code, which costs binary size [S12].

The lesson from IL2CPP is to make AOT the CI default from day one. Retrofitting AOT safety onto a reflection-heavy ecosystem cost Unity years. .NET 10 adds the `IsAotCompatible` assembly metadata and the `VerifyReferenceAotCompatibility` check [S12].

**Performance on iOS.** NativeAOT is RyuJIT's code generator run ahead of time. It should land close to desktop steady state, minus dynamic PGO (unverified for our workloads). Jitless V8 loses 40% (Speedometer) to 80% (Web Tooling) against its own JIT [S27]. So on the platform where performance is tightest, C# keeps its full optimizing compiler and TypeScript loses its JIT.

### 4. GC and frame times

- **CoreCLR's GC is not Unity's.**
  - Unity's Mono and IL2CPP runtimes historically used Boehm, a non-generational collector. Every allocation adds to whole-heap marking work, which is where Unity's "zero GC alloc" culture comes from.
  - CoreCLR is generational, with bump-pointer allocation in gen0. My probe allocated 12M short-lived 56-byte arrays across 600 "frames". That caused 132 gen0 collections with **1.41 ms of total pause, about 11 µs each** [M3].
  - The probe had a tiny heap and no surviving objects. Real pauses grow with heap size and survivor count. Keeping the managed heap small is the design lever.
- **GC mode:**
  - Use Workstation GC with background (concurrent) collection, the default for non-server apps.
  - DATAS has been on by default since .NET 9, but it adapts Server GC [S28], so it does not affect us.
  - Set `GCSettings.LatencyMode = SustainedLowLatency` during play. It suppresses blocking gen2 collections [S29].
  - NativeAOT ships the same GC.
- **Allocation-free patterns, enforced by tooling rather than by convention:**
  - Components are blittable structs stored in Rust columns.
  - Systems take components by `ref` over spans.
  - Managed objects are allowed only as `[Transient]` presentation caches.
  - Use `stackalloc`, `Span` and `ArrayPool`.
  - No LINQ or closures in code reachable from ticks (enforced by an analyzer).
  - Wrap each system in `GC.GetAllocatedBytesForCurrentThread()` and show an "alloc/frame" profiler column, the equivalent of Unity's GC Alloc column. Chart `GC.GetTotalPauseDuration()` as a frame-time track.
- **Compared with V8:** V8's GC is also generational and concurrent. But JavaScript has no value types, so vector math allocates unless it is written against typed arrays, which pushes the cost onto API design.

### 5. Sandboxing agent-written and user code

- **Modern .NET has no in-process hard sandbox.**
  - AppDomains are gone.
  - `Thread.Abort` is gone, so a runaway loop cannot be preempted.
  - There is no per-assembly memory quota.
  - A `StackOverflowException` kills the whole process.
- **s&box uses two layers.**
  - First, a Roslyn syntax "blacklist walker" runs after code generators.
  - Second, `AccessControl.VerifyAssembly` uses Mono.Cecil to check every referenced member against regex whitelist rules, grouped by area (`engine/Sandbox.Access/Rules/{BaseAccess,Reflection,Async,…}.cs`). It produces a `TrustedBinaryStream` [C7].
  - The s&box *editor* is not sandboxed.
  - Its new MCP `run_csharp` tool calls itself "trusted… full local process privileges… does not create undo steps". It compiles into a throwaway collectible ALC (`engine/Sandbox.Engine/Systems/Mcp/ScriptTools.cs`) [C7].
- **Space Station 14's Robust Toolbox** requires verifiable IL and checks referenced members against a whitelist. It bans `Activator`, so generic `new T()` must go through an engine factory, and it bans `stackalloc` [S23].
- **Mitigations available to snengine:**
  1. Dev-build instrumentation via Roslyn rewrites. Insert a budget check at loop back-edges. Insert `RuntimeHelpers.EnsureSufficientExecutionStack()` at method entry; it throws a *catchable* `InsufficientExecutionStackException` before a real stack overflow.
  2. Process isolation with OS limits (Job Objects, rlimits) for anything untrusted.
  3. s&box's whitelist verifier itself, which is MIT-licensed and portable.
- **`sn_run` works in C#, out of process.** Local agents already have shell access, so the realistic threat is mistakes, not malice. The requirements are therefore:
  - reviewability: writes happen only through the op log;
  - containment of hangs and crashes;
  - speed.

  A warm worker process meets all three. V8 isolates would be nicer in-process, but they are not worth a second language and runtime.

### 6. Agent quality in C#

The evidence that LLMs misuse Unity APIs is mostly anecdotal or vendor-reported. Examples include invented `LoadSceneAsync` overloads and the pre-Unity-6 `Rigidbody2D.velocity` instead of `linearVelocity`. Two 2026 papers show that compile-pass rate hides behavioural errors in generated Unity scenes [S30], which matches R03's GameLogicBench finding.

The precise hazard: a prior that **compiles against our API but means something different** is worse than one that fails to compile. Recommended stance:

- **Mirror Unity only where the behaviour is identical.** That covers inspector attributes (`[Range]`, `[Tooltip]`, `[Header]`, `[Min]`), `[FormerlySerializedAs]` (data migration on rename), and the component/prefab/scene vocabulary.
- **Rename deliberately wherever the behaviour differs:**
  - No `Update`, `FixedUpdate` or `Start` magic methods; Behaviours override `OnTick`, `OnFrame` and `OnSpawn` instead.
  - `Get<T>()` returns a `ref T`, not a nullable class like `GetComponent<T>()`.
  - Use `Entity` handles, not `GameObject`.
  - Use `ctx.Rng`, not `UnityEngine.Random`.
- **Roslyn analyzers with code fixes, including fixes registered against compiler errors.** For example, CS0246 on `MonoBehaviour`, or CS1061 on `.x` against `System.Numerics.Vector3`. `sn fix` and the MCP `apply` tool then repair Unity-isms mechanically. In-process diagnostics take 22–240 ms [M1], fast enough to run on every agent write.
- **Machine-readable API surface:**
  - A generated, typed SDK with XML docs.
  - `sn docs` / `sn schema` search over the reflection DB (R03).
  - A versioned API-stability policy.

  R06's lesson applies: agents write Godot 3 code for Godot 4, so API stability matters more than the choice of language.

### 7. Multiplayer interplay

C# can express R09's contract with attributes plus source generators.

- **Partial properties (C# 13/14) let a standard, non-rewriting generator implement property bodies.** So `[Property] public partial float Speed { get; set; }` can read and write ECS column memory directly. s&box needed a custom rewriting `[CodeGenerator]` meta-attribute to wrap `[Sync]` setters and `[Rpc]` methods (`Scene/Networking/HostSync.cs`, `Rpc.Attributes.cs`) [C7]. We don't.
- **The generator emits schema descriptors** (stable IDs, bounds, audience, rollback participation). These register with the Rust schema DB when the assembly loads.
- **Codecs, interpolation and history stay in Rust.** C# never defines a wire format.
- **Prediction needs synchronous, restorable state, and it holds by construction.** Commands and simulation features are `static` methods over `ref` components plus a context. They capture no managed state and use no `async` or `DateTime.Now`. Analyzers enforce this inside `[Simulation]` modules.
- **Hot reload in a multi-client session** pushes the delta or assembly to every peer; s&box does this [S9]. The update applies at a server-chosen tick and invalidates prediction history across that code epoch (R09 §7).

### 8. Unity-likeness without Unity's pitfalls

| Unity pitfall | Cause | snengine answer |
|---|---|---|
| Domain-reload stalls | Whole-domain teardown and reserialization | One ALC per module; state in ECS; L1 EnC |
| "Fake null" | Managed wrapper outlives its native object | Generation-checked `Entity`/`Ref<T>` with `IsAlive`/`TryGet`; no overloaded `==` |
| Static state leaking across play sessions | Statics survive (now the default, with Fast Enter Play Mode in 6.6 [S10]) | Analyzer error on mutable statics in gameplay modules; use `Resource`s instead |
| Magic-name methods | Reflection finds `Update` and friends by name | Virtual overrides and interfaces; an analyzer flags `void Update()` |
| Per-frame GC spikes | Boehm GC | Generational GC, struct components, alloc/frame profiler |
| IL2CPP problems found late | AOT is not the default build | NativeAOT in CI from day one; `IsAotCompatible` everywhere |

There are two authoring tiers over one component model:

- **Behaviours** are per-entity C# objects with MonoBehaviour-like ergonomics but no authoritative fields.
- **Systems** are batched queries for logic that touches many entities.

---

## Recommended design

### D1. Layered reload architecture

| Layer | Trigger | Mechanism | Target, save → visible (p50 / p95) | Basis |
|---|---|---|---|---|
| **L0** | Asset, style, UI markup, data values | Native reload; the CLR is not involved | ≤30 / ≤80 ms | R07 budgets |
| **L1** | Method, accessor or lambda body edits in a *hot* module, no debugger attached | Syntax-diff classifier → `EmitDifference` against the retained baseline → `ApplyUpdate` at the next tick boundary | ≤40 / ≤100 ms | 4–25 ms emit + <3 ms apply [M2], plus file watcher and one tick |
| **L2** (built after L1 is proven) | Additive edits: new methods, new types, new class fields | Same EnC path, gated on the reported runtime capabilities | ≤60 / ≤150 ms | Capability list [M2] |
| **L3** | Signatures, structs/components, schema attributes, any edit to an optimized module, debugger attached, or any L1/L2 refusal | Incremental Roslyn emit of the module (dependents rebuilt in parallel) → new collectible ALC → rebind systems and behaviours → Rust column migration → recreate behaviour objects → unload the old ALC asynchronously | ≤200 / ≤450 ms for modules up to ~5k lines | 42–389 ms emit by size [M1]; ALC unload 27–47 ms, off the critical path |
| **L4** | Rust engine change, leaked ALC, native crash, stack overflow | State-preserving process restart from a world snapshot (R07) | 1–5 s | R07 estimate |

**Module "heat".** This scheme keeps EnC's slowdown confined to the code actually being edited.

- **Modules.** Every gameplay module is a small reload unit. Folders map to assemblies through an asmdef-style `sn.module.toml`. The soft cap is about 5k lines, with a warning at 8k.
- **Cold by default.** Modules start *optimized* (cold).
- **Becoming hot.** The first edit to a cold module triggers one L3 rebuild *as Debug IL* and marks the module hot. After that, its body edits take L1.
- **Perf play.** "Play (perf)" recompiles hot modules as optimized in the background, at L3 cost each.
- **Visibility.** The profiler badges hot modules so the 4–24× penalty [M4] is never a surprise.
- **Fast-but-edited systems.** Systems that must stay fast while being edited live in their own module, or in Rust.

**Failure handling.** On a compile error, the last good code keeps running. On an L1 refusal, the coordinator silently escalates to L3.

```csharp
// Editor-side reload coordinator. Work runs on a background thread; results apply at tick boundaries.
sealed class ReloadCoordinator(ModuleCompiler compiler, ScriptHost host)
{
    public async Task OnSourcesChanged(ModuleId module, IReadOnlyList<SourceFile> changed)
    {
        var plan = await compiler.Classify(module, changed);          // conservative syntax diff (s&box-style)
        if (plan is BodyOnlyEdit body && host.IsHot(module) && !host.DebuggerAttached)
        {
            var delta = compiler.EmitDelta(module, body);             // EmitDifference vs retained EmitBaseline
            if (delta.Success) { host.AtNextTickBoundary(() => host.ApplyDelta(module, delta)); return; }
        }
        var build = await compiler.EmitModule(module, debugIL: host.IsHot(module) || plan.MakesHot);
        if (!build.Success) { host.Report(build.Diagnostics); return; } // last good code keeps running
        host.AtNextTickBoundary(() => host.SwapModule(module, build));  // new ALC, rebind, migrate columns
    }
}
```

### D2. Hosting and binding architecture

```
Rust snengine: bevy_ecs · schema DB · scheduler · renderer · netcode
   │  #[repr(C)] HostApiV1 { query_chunks, cmd_push, resource_ptr, register_schema, log, … }
   ▼
sn_script_init(host: *const HostApiV1, out: *mut ScriptApiV1)   ← [UnmanagedCallersOnly]: hostfxr (dev) / EntryPoint (AOT)
   ▲  ScriptApiV1 { run_system(id, chunks, n, tick), on_command(id, ctx), on_reload(module), … }
Sn.Runtime (hand-written C#, AOT-safe)  +  Sn.Generated (C# structs/wrappers generated from the Rust schema DB)
   ▲
Game modules (C#): one collectible ALC per module in dev · one NativeAOT image when shipping
```

```rust
#[repr(C)]
pub struct Chunk { pub len: u32, pub columns: *mut *mut u8, pub entities: *const u64 }

#[repr(C)]
pub struct HostApiV1 {
    pub abi_version: u32,
    /// Writes up to `cap` archetype chunks matching query `q` into `out`; returns the count.
    pub query_chunks: extern "C" fn(q: QueryId, out: *mut Chunk, cap: u32) -> u32,
    /// Appends a blittable structural command (spawn/insert/remove/despawn) to a buffer applied at a sync point.
    pub cmd_push: extern "C" fn(buf: CmdBufId, bytes: *const u8, len: u32),
    /// Registers C#-declared component/command descriptors; returns a schema version.
    pub register_schema: extern "C" fn(json: *const u8, len: u32) -> u32,
    pub log: extern "C" fn(level: u8, msg: *const u8, len: u32),
}
```

```csharp
// User code: a system over components.
public sealed partial class Gravity : SimSystem
{
    [Query] static void Run(ref Velocity v, in Mass m, in Tick t) => v.Value.Y -= 9.81f * t.Dt;
}

// Generated: one boundary crossing per chunk, spans over Rust columns, faults contained.
unsafe partial class Gravity
{
    [UnmanagedCallersOnly]
    static int __run(Chunk* chunks, uint count, Tick* tick)
    {
        try
        {
            for (uint c = 0; c < count; c++)
            {
                var vel = new Span<Velocity>(chunks[c].Columns[0], (int)chunks[c].Len);
                var mass = new ReadOnlySpan<Mass>(chunks[c].Columns[1], (int)chunks[c].Len);
                for (int i = 0; i < vel.Length; i++) Run(ref vel[i], in mass[i], in *tick);
            }
            return 0;
        }
        catch (Exception e) { SnDiagnostics.SystemFault(typeof(Gravity), e); return 1; }
    }

    // Read/write sets let Bevy's scheduler run C# systems in parallel with Rust ones.
    internal static readonly SystemDesc Desc = new(writes: [typeof(Velocity)], reads: [typeof(Mass)]);
}
```

Binding rules:

- Entities are `u64` (index + generation).
- Strings cross the boundary as UTF-8 spans or interned IDs.
- Structural changes go through command buffers, applied at sync points.
- `GCHandle`s exist only for editor and UI objects, never for simulation state.
- The schema DB stays the single source of truth (R01):
  - Rust-declared components generate C# structs.
  - C#-declared components register their layout and descriptors back as dynamic components (bevy_ecs `ComponentDescriptor`).

### D3. Gameplay API shape

```csharp
[Component("game.Vitality")]
public partial struct Vitality
{
    [Range(0, 100), Replicate(Audience.Relevant)] public ushort Hp;
    [Range(0, 9),   Replicate(Audience.Owner)]    public byte Medkits;
}

// Per-entity object with Unity-like ergonomics; authoritative state lives in the generated ECS column Door.Data.
public sealed partial class Door : Behaviour
{
    [Property, Tooltip("Degrees")] public partial float OpenAngle { get; set; }
    [Property, Replicate]          public partial bool IsOpen { get; set; }
    [Transient] AudioClipHandle _creak;   // presentation-only cache: not saved, not replicated, dropped on reload

    protected override void OnSpawn() => _creak = Assets.Load<AudioClip>("sfx/creak");

    protected override void OnTick(in Tick t)
    {
        ref var tr = ref Get<Transform>();
        var target = Quaternion.CreateFromAxisAngle(Vector3.UnitY, IsOpen ? MathX.Rad(OpenAngle) : 0f);
        tr.Rotation = Quaternion.Slerp(tr.Rotation, target, t.Dt * 4f);
    }
}

// Prediction-safe by construction: static, synchronous, ref components + context only.
[Command("game.UseMedkit"), Expose(ExecuteOn.Authority, Caller.Owner, RatePerSecond = 2)]
public static partial class UseMedkit
{
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, ref Vitality v)
    {
        if (v.Hp == 100 || v.Medkits == 0) return ctx.Reject("cannot_heal");
        v.Medkits--;
        v.Hp = (ushort)Math.Min(100, v.Hp + 25);
        ctx.Cue(new MedkitUsed(actor));
        return CommandResult.Ok;
    }
}
```

Behaviour objects hold no authoritative fields. An L3 reload therefore just recreates them (10k objects is roughly a millisecond, unmeasured) and calls `OnReload`. Plain instance fields on a `Behaviour` are an analyzer error unless they are `[Transient]`.

### D4. Analyzer set (ships with the SDK; code fixes wherever the change is mechanical)

| ID | Rule |
|---|---|
| SN0001 | Unity-isms: `using UnityEngine`, `MonoBehaviour`, `void Update()`/`Start()`, `GetComponent<T>()`, `.x/.y/.z` → fix to the snengine equivalent |
| SN0100 | Mutable static field in a gameplay module → use a `Resource` |
| SN0101 | Instance field on a `Behaviour` without `[Transient]` |
| SN0200 | In `[Simulation]` code: `async`/`Task`, `DateTime.Now`, `System.Random`, I/O, threads, captured lambdas in commands |
| SN0300 | Allocation in tick-reachable code (LINQ, closures, class `new`, boxing): warning, linked to the profiler |
| SN0400 | AOT/trim-unsafe API (`Assembly.Load`, `Reflection.Emit`, unannotated reflection): error outside editor modules |

### D5. Trust tiers and `sn_run`

| Code | Where it runs | Controls |
|---|---|---|
| Project gameplay and editor code, written by humans or agents | In process | Analyzers; loop and stack guards in dev builds; faults isolated per system |
| `sn_run` agent scratch scripts | **Warm out-of-process worker**, pooled, with one spare kept warm (respawn ≈ runtime init 93 ms [M3]) | Compiled only against the `Sn.Agent` facade (queries and typed commands over the Engine Protocol). IL whitelist ported from s&box `Sandbox.Access`. The worker is killed on wall-clock or memory limits. All writes form one reviewable transaction |
| Marketplace / UGC code | Clients: in process, compiled from source and whitelisted (s&box model). Servers: out of process, or not at all | Wasm later for hostile mods that need native speed (R06) |

```csharp
// An sn_run body as an agent sends it (usings and the Sn.Agent facade are implied).
var closed = await World.Query<Door>().Where(d => !d.IsOpen).ToListAsync();
foreach (var d in closed) await Commands.SetProperty(d.Entity, (Door x) => x.OpenAngle, 110f);
return new { changed = closed.Count };   // becomes one Txn with the agent's "why"
```

### D6. Build order (nothing dropped)

1. **Weeks 0–6, prototype gate.** Build the Rust host (`netcorehost`), the generated ABI, and one C# module of systems over bevy_ecs tables. Get L3, then L1, working. Log p50/p95 per save. Pass criteria are the D1 targets on Islam's machine.
2. Behaviours, analyzers SN0001–SN0400, the `sn_run` worker, and generated debugger launch profiles.
3. A nightly NativeAOT desktop export in CI, starting in month 2, so AOT-incompatible code never piles up.
4. iOS: a NativeAOT static library linked into the Rust app (Godot's recipe). Then Android NativeAOT on bionic, with a Mono or CoreCLR runtime pack as the fallback.
5. Later items:
   - L2 additive EnC.
   - On-device dev reload: a JIT runtime on Android; the .NET 11+ CoreCLR interpreter on iOS (unverified for custom hosts).
   - UGC whitelisting.

---

## C# vs TypeScript head-to-head

| Criterion | C# (.NET 10: CoreCLR in dev, NativeAOT when shipping) | TypeScript (V8 via deno_core) | Edge |
|---|---|---|---|
| Agent fluency | Top tier; Unity-flavoured priors, mitigated by D4 | Top tier; largest corpus | TS, slightly |
| Static types | Nominal, not erased; value types and `ref` | Structural and erased | C# |
| Body-edit iteration | Measured 4–25 ms delta + <3 ms apply, but the code runs unoptimized while hot | Module swap at full JIT speed; R07 budgeted 100 ms–1 s, probably faster in practice (not measured) | Latency: tie or C#. Perf while iterating: TS |
| Structural-edit iteration | Measured 42–389 ms emit (by module size) + rebind | Module swap; schema migration is identical | TS, slightly |
| ECS data access | Spans over Rust columns: 1.74 vs 1.09 ns/entity, measured | Typed arrays/DataView or per-field ops; no structs | C# |
| Boundary cost | 1–4 ns/call, measured | V8 Fast API calls exist; deno ops are heavier (not measured) | C# |
| iOS performance | Full AOT, supported since .NET 9 | Jitless: −40% to −80% [S27] | **C#** |
| Android | NativeAOT experimental; Mono/CoreCLR fallbacks | V8 JIT allowed, but there is no rusty_v8 Android prebuilt, so we would build V8 ourselves (R08) | Tie |
| Web (bonus tier) | Gap | Native browser JS | **TS** |
| GC and frame time | Generational GC + value types → small heaps | Generational; more allocation pressure | C# |
| In-process sandboxing | None; whitelist + out-of-process worker | Isolates, heap caps, `TerminateExecution` | **TS** |
| Hang / stack-overflow containment | Instrumentation + process boundary | Built in | TS |
| Debugging | VS / Rider / VS Code; mixed native+managed on Windows | V8 inspector → VS Code / Chrome | C#, slightly |
| Native non-web UI logic | s&box precedent: Razor components and stylesheets on native panels (`engine/Sandbox.Engine/Systems/UI`, `WorldPanel`) [C7] | Restricted TSX DSL (R05) | Tie |
| Multiplayer declarations | Attributes + partial properties + standard generators | Schema builders (types are erased) | C# |
| Owner fit | Unity developer: native | TS developer: native | Tie |
| Reusable prior art | s&box (MIT): compile, hotload, whitelist and UI. Godot/Flax hosting patterns | Puerts, deno_core | **C#** |
| Embedding and shipping | hostfxr in dev + the NativeAOT compiler per platform; 1.03 MB AOT runtime, measured | rusty_v8 static libraries: ~20 MB gzipped for iOS, ~40 MB for desktop (R08) | C# |

**Verdict: C#.** It wins where snengine's pillars are expensive to fix later.

- **Unreal-level performance:** value types over native memory, full AOT on iOS.
- **Unity-level ease:** the owner and the largest game-dev audience already think in C#.
- **Near-instant iteration:** achieved once state lives in ECS and modules stay small.

TypeScript wins on sandboxing, web and corpus. Each of those has a containment strategy (process boundary, bonus-tier web, analyzers), and none sits in the core loop.

**I would reverse this verdict if any of these happen:**

1. The week-6 gate misses L1 p95 ≤100 ms or L3 p95 ≤450 ms on Islam's machine.
2. Linking NativeAOT into the Rust app fails on iOS or Android, with no credible fix.
3. snbench shows C# agent task success more than ~10 points below a TypeScript control, even after analyzers and docs.
4. Web export, or an in-process untrusted-UGC platform, becomes a first-tier requirement.

---

## Open questions / risks

- **EnC with a debugger attached.** Can IDE-driven EnC work against an embedded host? Visual Studio lists it as unsupported [S2], and the `COMPLUS_ForceENC` attach path is untested. Until that's verified, debug sessions use L3.
- **Classifier correctness.** A missed rude edit can corrupt the process silently [M2]. Ship body-only first (proven by s&box), and fuzz the classifier with recorded edit corpora.
- **Perf while hot.** A 24× penalty on vector math [M4] can make play-in-editor misleading. The UX must make heat visible and offer a one-click "cool all".
- **Android NativeAOT** is still experimental [S12]. Using the CoreCLR Android runtime pack from a custom (non-MAUI) host is unverified.
- **ALC leaks.** Third-party libraries that cache `Type`s pin ALCs (see Flax's TypeDescriptor workaround [C2]). Gate NuGet dependencies in gameplay modules, detect leaks, and fall back to L4.
- **Scale.** Roslyn memory, and the cascade of dependent-module rebuilds, need measuring on a 100k+-line project (unverified).
- **UI authoring language.** R05's restricted-TSX recommendation needs a C# counterpart, for the UI track to decide. Options: restricted Razor-like markup (s&box's proven path), or XML plus stylesheets with generated C# controller bindings.
- **Runtime upkeep.** We bundle and pin the .NET runtime, ship a reference pack for Roslyn, and ship the NativeAOT compiler toolchain for exports. Plan to track LTS releases: .NET 10 is supported until 2028-11-14 [S17], and .NET 11 (STS) is due November 2026 [S14,S17].

---

## Final summary

**Recommendation.** C# 14 on .NET 10 LTS, moving to each next LTS. One language for gameplay, UI logic, editor extensions and `sn_run`; no JavaScript runtime. Reversal conditions are listed under the verdict above.

**Reload architecture (targets, p50 / p95):**

| Layer | Target |
|---|---|
| L0 data / UI | ≤30 ms |
| L1 EnC body edits (hot modules only) | ≤40 / ≤100 ms |
| L2 additive EnC (built later) | ≤60 / ≤150 ms |
| L3 ALC module swap (modules ≤5k lines) | ≤200 / ≤450 ms |
| L4 snapshot restart | 1–5 s |

**Hosting and binding.**
- `netcorehost`/hostfxr in the editor and dev builds; NativeAOT images when shipping.
- One generated `#[repr(C)]` function-pointer ABI in both directions.
- Chunk-batched queries over spans.
- Command buffers for structural changes.
- The Rust schema DB is the single source of truth.
- Exceptions are contained at every boundary entry.

**Platform matrix for scripting:**

| Platform | Editor / dev | Code reload | Shipping runtime | Status |
|---|---|---|---|---|
| Windows | CoreCLR JIT + in-process Roslyn | L0–L4 | NativeAOT (or CoreCLR + ReadyToRun for games that want desktop C# mods) | Supported |
| Linux | Same | L0–L4 | NativeAOT | Supported |
| macOS | Same | L0–L4 | NativeAOT | Supported |
| iOS | Desktop editor → device builds | Rebuild (interpreter-based dev reload unverified) | NativeAOT static library (`PublishAotUsingRuntimePack`) | Supported runtime; Godot recipe |
| Android | Desktop editor → device builds | Rebuild (JIT dev runtime later) | NativeAOT (bionic), with a Mono/CoreCLR fallback | Experimental: gate early |
| Web (bonus) | — | — | Nothing viable today | Gap |

**The 5 scripting decisions hardest to change later**

1. **Authoritative state lives in engine ECS columns, never in the managed heap.** Reload, save, replication, prediction and agent introspection all depend on it.
2. **The Rust↔C# ABI and its granularity:** a generated function-pointer table, chunk-batched spans, no per-entity calls. Every API inherits this shape.
3. **The public API's naming and lifecycle model:** Behaviours plus Systems, explicit overrides, Unity names only where the semantics match. Docs, user code and agent priors all harden around it.
4. **Module topology and the reload contract:** small assemblies, no mutable statics, one collectible ALC per module, hot/cold heat. Retrofitting module boundaries onto a monolith is a rewrite.
5. **AOT-first discipline:** no runtime code generation or dynamic loading outside the editor, and generators instead of reflection. Every package must comply from day one, or iOS and Android shipping breaks.

---

## Sources

**Web**
- [S1] Roslyn, *EnC Supported Edits*: https://github.com/dotnet/roslyn/blob/main/docs/wiki/EnC-Supported-Edits.md
- [S2] Microsoft Learn, *Supported code changes (C#)*, updated 2026-10-08: https://learn.microsoft.com/en-us/visualstudio/debugger/supported-code-changes-csharp
- [S3] `MetadataUpdater.ApplyUpdate` API docs: https://learn.microsoft.com/en-us/dotnet/api/system.reflection.metadata.metadataupdater.applyupdate
- [S4] dotnet/runtime `src/coreclr/vm/assemblynative.cpp` (`ApplyUpdate`: `NotSupported_DebuggerAttached`, `InvalidOperation_AssemblyNotEditable`): https://github.com/dotnet/runtime/blob/main/src/coreclr/vm/assemblynative.cpp
- [S5] Roslyn `RudeEditKind.cs`: https://github.com/dotnet/roslyn/blob/main/src/Features/Core/Portable/EditAndContinue/RudeEditKind.cs
- [S6] Roslyn `ExternalAccess/HotReload/Api/HotReloadService.cs` (`internal sealed`): https://github.com/dotnet/roslyn/blob/main/src/Features/ExternalAccess/HotReload/Api/HotReloadService.cs
- [S7] s&box source (MIT): https://github.com/Facepunch/sbox-public
- [S8] s&box docs, *Getting started* / *Code*: https://sbox.game/dev/doc/getting-started/ · https://sbox.game/dev/doc/code/
- [S9] Facepunch wiki, s&box *Games/GettingStarted* (code pushed to connected clients): https://wiki.facepunch.com/sbox/Games/GettingStarted
- [S10] Unity, *CoreCLR, Scripting, and Serialization Update – June 2026*: https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299
- [S11] Unity, *CoreCLR, Scripting, and ECS Status Update – March 2026*: https://discussions.unity.com/t/coreclr-scripting-and-ecs-status-update-march-2026/1711852
- [S12] Microsoft Learn, *Native AOT deployment overview* (platform table, limitations, `IsAotCompatible`), updated 2026-07-27: https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/
- [S13] Microsoft Learn, *Native AOT support for iOS-like platforms*: https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/ios-like-platforms/
- [S14] .NET Blog, *.NET MAUI moves to CoreCLR in .NET 11* (2026-05-13): https://devblogs.microsoft.com/dotnet/dotnet-maui-moves-to-coreclr-in-dotnet-11/
- [S15] Microsoft Learn, *Interpreters on iOS and Mac Catalyst* (.NET 11: composite R2R + CoreCLR interpreter), updated 2026-10-01: https://learn.microsoft.com/en-us/dotnet/maui/macios/interpreter?view=net-maui-11.0
- [S16] dotnet/macios, *.NET 11.0.1xx RC 2* release notes: https://github.com/dotnet/macios/releases/tag/dotnet-11.0.1xx-rc2-12212
- [S17] .NET downloads and support (11.0 RC1 2026-09-08; .NET 10 supported to 2028-11-14): https://dotnet.microsoft.com/en-us/download/dotnet
- [S18] Uno Platform, *Native AOT … five platforms* (secondary): https://platform.uno/blog/native-aot-in-uno-platform-faster-startup-five-platforms/
- [S19] Godot docs, *C# platform support*: https://docs.godotengine.org/en/stable/tutorials/scripting/c_sharp/index.html
- [S20] Godot 4.2 docs, *C# basics* (hot-reload state limits): https://docs.godotengine.org/en/4.2/tutorials/scripting/c_sharp/c_sharp_basics.html
- [S21] JetBrains Rider support thread on Godot C# hot reload: https://rider-support.jetbrains.com/hc/en-us/community/posts/22764175527826
- [S22] Flax, *Flax Facts #16: scripts hot reload*: https://flaxengine.com/blog/flax-facts-16-scripts-hot-reload/
- [S23] Space Station 14 docs, *Robust Toolbox sandboxing*: https://docs.spacestation14.com/en/robust-toolbox/sandboxing.html
- [S24] `netcorehost` 0.22.0 (crates.io, 2026-07-26): https://github.com/OpenByteDev/netcorehost
- [S25] Microsoft Learn, *Write a custom .NET host*: https://learn.microsoft.com/en-us/dotnet/core/tutorials/netcore-hosting
- [S26] Unity Manual, *Scripting restrictions* (IL2CPP/AOT): https://docs.unity3d.com/Manual/scripting-restrictions.html
- [S27] V8 blog, *Jitless V8*: https://v8.dev/blog/jitless
- [S28] .NET Blog, *Preparing for the .NET 10 GC* (DATAS): https://devblogs.microsoft.com/dotnet/preparing-for-dotnet-10-gc/
- [S29] Microsoft Learn, *GC latency modes*: https://learn.microsoft.com/en-us/dotnet/standard/garbage-collection/latency
- [S30] Liu & Tatar, *Compiler error census for single-pass LLM Unity scenes* (arXiv 2607.10187): https://arxiv.org/abs/2607.10187 · *Mage: multi-axis evaluation of LLM-generated game scenes* (arXiv 2605.07342): https://arxiv.org/pdf/2605.07342
- [S31] Microsoft Learn, *What's new in .NET MAUI for .NET 11* (Android: CoreCLR default; NativeAOT experimental opt-in): https://learn.microsoft.com/en-us/dotnet/maui/whats-new/dotnet-11
- [S32] Microsoft Learn, *Native AOT deployment on iOS and Mac Catalyst* (no experimental label; diagnostics limits), updated 2026-10-01: https://learn.microsoft.com/en-us/dotnet/maui/deployment/nativeaot

**Code (read-only)**
- [C1] `.refs/flax/Source/Engine/Scripting/Scripting.cpp:732-803`: `Scripting::Reload` (release objects, unload modules, recreate ALC, GC, load)
- [C2] `.refs/flax/Source/Engine/Engine/NativeInterop.Unmanaged.cs:976-1125`: collectible ALC creation; unload cache-clearing, including the `TypeDescriptor` workaround
- [C3] `.refs/flax/Source/Tools/Flax.Build/Build/DotNet/DotNetAOT.cs:15-35` (`ILC`, `MonoAOTDynamic`, `MonoAOTStatic`); `.refs/flax/Source/Editor/Cooker/Platform/iOS/iOSPlatformTools.cpp:108-110` (iOS → `MonoAOTDynamic`); `.refs/flax/Source/Editor/Cooker/PlatformTools.h:90-93` (default `None`)
- [C4] `.refs/godot/modules/mono/csharp_script.cpp:625-700`: `reload_assemblies` (editor only; serializes instances and delegates)
- [C5] `.refs/godot/modules/mono/editor/Godot.NET.Sdk/Godot.NET.Sdk/Sdk/iOS.props:3-4` (`PublishAot`, `PublishAotUsingRuntimePack`); `Sdk/Android.props:3` (`UseMonoRuntime`)
- [C6] `.refs/godot/modules/mono/mono_gd/gd_mono.cpp` (hostfxr hosting); `mono_gd/gd_mono_cache.h:71` (`ManagedCallbacks`); `glue/GodotSharp/GodotSharp/Core/NativeInterop/NativeFuncs.cs` (`delegate* unmanaged` table)
- [C7] s&box @ `f7bedf4` (temp clone):
  - `engine/Sandbox.Compiling/Compiler/Compiler.Build.cs:165-290` (incremental Roslyn, whitelist, emit)
  - `engine/Sandbox.Generator/Processors/ILHotload.cs` (body-only classifier)
  - `engine/Sandbox.Hotload/ILHotload/ILHotload.cs` (MonoMod detours)
  - `engine/Sandbox.Hotload/{Hotload,UpdateReferences}.cs` (instance upgrade)
  - `engine/Sandbox.Engine/Services/Packages/PackageManager/PackageLoader.cs:600-630` (`TryFastHotload`, editor only)
  - `engine/Sandbox.Access/**` (Cecil whitelist)
  - `engine/Sandbox.Engine/Systems/Mcp/ScriptTools.cs` (`run_csharp`)
  - `engine/Sandbox.Compiling/CompilerConfiguration.cs:46-50` (dev builds default to Debug)
  - `engine/Sandbox.Engine/Scene/Networking/{HostSync,Rpc.Attributes}.cs` (`[CodeGenerator]` wrapping)
  - `engine/Sandbox.Engine/Systems/UI/**` (Razor, styles, layout, `WorldPanel`)

**Measurements** (scratch code in `%TEMP%\r10\bench`): M1/M2 `roslynbench`, M3 `rusthost` + `gamelib`, M4 `encperf`.
