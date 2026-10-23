# Ara.CompileServer

The warm out-of-process compile server (A13): Roslyn never runs in the game process, so
its CPU and GC pauses never stall ticks. One process serves a host over a named pipe
(`Protocol.cs`); it runs below normal priority and exits with its parent.

## Pieces

- `ModuleSession`: one module. Retained user compilation + generator driver + final
  compilation, EnC baseline, heat, file watcher (1 ms spin settle), worker thread.
  - Initial build (cold = Release IL unless `hot`), prewarm of the L1 and L3 paths.
  - Edit: classify; body-only edits of a hot module → `EmitDifference` (L1, no generator
    run); anything else → generator + full emit (L3). A cold module's first edit is one
    L3 that makes it hot. Errors (compiler, generator, analyzers) are sent as diagnostics
    and the last good state stays applied. Logs per-phase timings at info level.
- `EditClassifier`: S2's conservative body-only diff, plus: constructors/initializers,
  body edits in primary-constructor types (a new capture is a new field), state machines,
  closures, doc-comment changes and generated-code changes escalate.
- `ModuleCompiler`: references (ref pack), options, emit, EnC baselines from the PDB.
- `AnalyzerSet`: the Ara analyzers over changed trees. `Diagnostics`: Roslyn → ara-diag JSON
  (`CSxxxx` errors become ARA0900 with the id in the message).

## Gotchas

- Generator output is assumed identical for body edits (see Ara.Generators). If that
  ever changes, L1 must run the generator again.
- Module size: info past 5k lines (ARA0902), warning past 8k (ARA0903).

## Testing

`dotnet test tests/Ara.CompileServer.Tests` (classifier cases; sessions: cold → hot L3 →
L1, errors keep the last good state, structural/doc edits, analyzer errors).
