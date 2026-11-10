# Query tag discovery — Mac, 2026-11-10

Query extraction asked whether a component had any instance fields by materializing every reflected field, reading its documentation and attributes, and then counting the list. Scalar and batch queries now stop at the first instance field. The predicate is identical: non-static, non-const `IFieldSymbol`, including implicit backing fields. Full schema extraction and required-documentation diagnostics retain their normal path. No new cache, gameplay state, registration order or generated code is introduced.

## Actual compiler allocation comparison

Baseline is main `5794174e8`; candidate changes only the two tag checks and adds `Schema.HasFields`. Both complete SDK closures use the same compiler sources, runtime, analyzer and common dependency bytes. Generator DLL/PDB and the compiler DLL/PDB rebuilt against them differ; their full hashes are retained. Same absolute fixture path, 8 modules × 128 generated component/query scripts, one edited script per module per request, four fresh processes ABBA, five warmups and ten measured requests per process. Production generators/analyzers stay enabled. Runtime: observed CoreCLR 10.0.7, macOS arm64.

| Counter | Previous | Field presence check |
| --- | ---: | ---: |
| Median allocated bytes/request, 20 measured requests/arm | 700,415,616 | 676,407,944 |
| Minimum–maximum allocated bytes/request | 698,462,568–703,431,088 | 673,951,024–677,654,056 |
| Median final live heap, two processes/arm | 168,845,768 | 169,662,692 |

Allocation falls **24,007,672 B/request (3.43%)** in this fixture. Final heap increases 816,924 bytes at the two-process median; those overlapping observations do not establish a retention improvement. Individual process medians and final heaps are retained in the JSON packet. The comparison is not a latency, p95, peak-RSS, time-to-Play or ECS-performance result. External load and startup-hook overhead invalidate clocks. Final heap uses collect/finalize/collect with compiler state and last reply rooted; the last request contains that forced GC. Logging is outside allocation counter boundaries but can shift collection phase.

All 60 requests completed, with 360 corresponding module comparisons matching exact PE, PDB and canonical-schema hashes. Complete SDK/hook/harness hashes remain unchanged. The packet includes every request's allocation/GC counters, raw unaccepted clocks, emitted hashes and loaded runtime image. Baseline/candidate sources emit identical code; this change affects generator work only.

Audited Release solution build: zero warnings/errors. All **335 .NET tests** pass (233 compiler, 35 generator, 30 analyzer, 28 runtime, 9 worker). Ten native scripting tests pass across batch queries, project publication, project lifecycle, compile lifecycle and reload levels. New generator cases cover scalar/batch empty tags, static/const-only tags, public/private data fields, undocumented fields and auto-property backing fields. Missing docs still report ARA0703; fieldless parameters still report ARA0702.

Reproduce allocation counters using complete frozen SDK directories and the existing startup hook:

```sh
python3 benchmarks/iteration/retained_gc.py \
  --baseline /absolute/baseline-sdk/Ara.CompileServer.dll \
  --candidate /absolute/candidate-sdk/Ara.CompileServer.dll \
  --hook /absolute/Hook.dll --dotnet-host /absolute/dotnet \
  --modules 8 --files 128 --warmups 5 --samples 10 \
  --output /absolute/new-empty-output
```

The hook source and build instructions are in `project_gc/`; plotting uses `plot_query_tags.py`. Diagnostic tools are optional and are not engine build dependencies.

## How the candidate was chosen

An actual warmed compiler trace covered five edited requests using local `dotnet-trace 10.0.750501`, `dotnet-sampled-thread-time,gc-verbose`, a 64 MB buffer and 120-second bound. Trace and compiler exited cleanly; frozen SDK hashes matched; TraceLog reported zero lost events. Managed thread-time samples include waits, so their percentages are not CPU shares. Allocation ticks are sampled weights, not exact per-site allocated bytes. The tool's package signature was not independently verified (installation reported skipped NuGet signature verification). The initial capture waited for a non-TTY progress banner and captured no edits; it is excluded from attribution and remains under `target/`.

Nearest Ara/Roslyn frame attribution places 324,538,304 weighted bytes / 3,041 ticks at `Docs.Tag` across the five requests. Binding, nullable flow and PE emission also appear prominently. Field materialization called that documentation path only to decide whether a query parameter was a tag. This profile selected the field-presence experiment; only the subsequent exact counters support the 3.43% result. Full nullable analysis remains enabled. See [dotnet-trace documentation](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/dotnet-trace) for sampling limitations.

Two earlier source-generation experiments were rejected and restored before this comparison:

| Prototype | Median allocated B/request, before → prototype | Final heap median B, before → prototype | Decision |
| --- | ---: | ---: | --- |
| Named registration setter method groups | 700,258,444 → 699,639,652 | 169,059,872 → 173,954,500 | Reject: tiny allocation difference, higher heap, no admitted latency |
| Explicit arrays instead of collection expressions | 699,219,976 → 716,953,536 | 168,618,564 → 171,297,328 | Reject: more allocation/heap; three source-text assertions failed |

These used four fresh processes, three warmups and five measured requests/process. Their schema-only exploratory checks do not prove generated-code equivalence; they never passed the full quality gate and are not shipped. Their decision receipts are retained in the public packet. Raw local evidence, trace, parser and rejected diffs remain under `target/script-overnight-20261110/`; hashes bind the accepted trace and test logs without committing the 33 MB trace.

Dependency reverse pairs and the historical retained-state p95 conclusion remain unresolved. The previously published rejected attempts are unchanged. The next profile-led candidate is documentation parsing itself, with exact XML/diagnostic parity required before any adoption.
