# Ara.CompileServer

The warm out-of-process compile server (A13): Roslyn never runs in the game process, so
its CPU and GC pauses never stall ticks. One process serves a host over a named pipe
(`Protocol.cs`, v2) and exits with its parent.

## Pieces

- `Coordinator`: the scheduler. Modules and their reference graph (a module is watched
  after the modules it references), the work queues and the one worker thread.
  - Human work (watcher saves, heat, rebuilds) runs first, in order, at normal process
    priority; queued saves of one module merge.
  - Agent edits (`Change` with `OriginAgent`, and watcher events on paths an agent claimed)
    coalesce per module until quiet for `AgentQuiet` (150 ms), run only with no human work
    queued, below normal priority, and are cancelled and requeued when a human edit arrives.
    A human save of the same module absorbs them (D20).
  - An image (L3) cascades to every dependent in dependency order: relink (same image and EnC
    chain, `Escalation.Relink`) when the dependency's public surface is unchanged, rebuild
    (`Escalation.Dependency`) otherwise. The cascade is one `Reload` message, or nothing
    applies if a dependent fails (`ARA0925` on both modules).
  - Watches `Ara.Runtime.dll` (the host's default context): if it changes, sends `Restart`
    (L4) once.
- `ModuleSession`: one module. Its committed `ModuleState` (immutable: compilations,
  generated sources, trees and texts, EnC baseline, heat, generation, public surface hash,
  the dependency surfaces it was compiled against) and the generator driver. `Initial`,
  `Compile`, `Full` (heat/rebuild), `Cascade` and `Rebind` return an `Outcome` holding the
  next state; `Commit` adopts it.
  - Compile: classify; L1/L2 → `EmitDifference` (the generator runs only for inserts and doc
    edits, and any change in its output is L3); L3 → generator + full emit with the analyzers
    in parallel. Errors (compiler, generator, analyzers) are diagnostics; nothing commits.
- `EditClassifier`: member-level diff. L1 bodies keep S2's rules (no closures, state machines,
  constructors, primary-constructor types). L2 inserts: methods/operators/non-auto properties,
  class fields and auto-properties without initializers, consts, new non-generic non-partial
  types, lambdas inside new members, each gated on the runtime's `EncCapabilities`
  (`Escalation.Capability`). Everything else is L3.
- `ModuleCompiler`: shared framework references, options, emit, EnC deltas (`isAddedSymbol`
  for inserts), baselines from the PDB, `Surface` (hash of public/protected declarations).
- `ModuleWatcher`: one pump thread for every module folder (1 ms spin settle).
- `AnalyzerSet`: the Ara analyzers over changed trees, with the module's API profile
  (`build_property.AraModuleProfile`). `Diagnostics`: Roslyn → ara-diag JSON.

## Runtime config

`TieredCompilation=false`: every Roslyn method is fully JIT-optimized on first use (the
initial build and the prewarm pay for it). With tiering, emit paths that run once per edit
sat at tier 0 or in PGO instrumentation for many edits; a 5.2k-line L3 emit took p50
167-207 / p95 226-314 ms with tiering, 86-100 / 107-137 ms without (interleaved runs).

## Gotchas

- Generator output is assumed to depend only on declarations, attributes and doc comments.
- New L1/L2 allowances are reviewed changes: a missed rude edit crashes the game process.
- Module size: info past 5k lines (ARA0902), warning past 8k (ARA0903).
- Generations: initial 0, +1 per image or relink; deltas keep the generation.

## Testing

`dotnet test tests/Ara.CompileServer.Tests`: `ClassifierTests` (the L1/L2/L3 edit matrix,
capabilities, new/removed files), `SessionTests` (cold → hot, L1, L2, errors, Ara
declarations, unknown references, L4), `CascadeTests` (local L1, relink, rebuild, breaking
edits, dependents seeing L2 members), `SchedulingTests` (agent coalescing and priority).
