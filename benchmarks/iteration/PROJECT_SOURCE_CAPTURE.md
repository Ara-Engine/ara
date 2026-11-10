# Project source capture — Mac, 2026-11-10

Preparation previously called `File.ReadAllText` for every source, allocating fresh strings even when the module already retained identical parsed text. The warm path now fully reads and decodes each file, reusing the retained string only after exact EOF equality. Hashing still covers the captured text. Cold reads keep their existing path; file sharing, BOM/fallback behavior and error semantics remain unchanged. No new source cache is retained.

Four fresh processes run baseline → candidate → candidate → baseline on CoreCLR 10.0.7. Each case uses eight warmups and twenty measured capture sets per process. These are synchronous per-thread allocation counters, with file writes, expected hashes and validation outside the counter. All frozen SDK/helper/runtime/source hashes remain unchanged.

| UTF-16 units/file | Scenario | Edit | Baseline B/file | Candidate B/file | Change |
| ---: | --- | --- | ---: | ---: | ---: |
| 384 | cold | none | 9,822.50 | 9,822.75 | +0.00% |
| 384 | unchanged | none | 9,822.50 | 3,990.75 | -59.37% |
| 384 | one_changed | early | 9,822.50 | 4,041.75 | -58.85% |
| 384 | all_changed | early | 9,822.50 | 5,622.75 | -42.76% |
| 384 | one_changed | middle | 9,822.50 | 4,041.75 | -58.85% |
| 384 | all_changed | middle | 9,822.50 | 5,622.75 | -42.76% |
| 384 | one_changed | late | 9,822.50 | 4,041.75 | -58.85% |
| 384 | all_changed | late | 9,822.50 | 5,622.75 | -42.76% |
| 384 | one_changed | replacement | 9,822.50 | 4,041.75 | -58.85% |
| 384 | all_changed | replacement | 9,822.50 | 5,622.75 | -42.76% |
| 16,384 | cold | none | 84,870.50 | 84,870.75 | +0.00% |
| 16,384 | unchanged | none | 84,870.50 | 4,062.75 | -95.21% |
| 16,384 | one_changed | early | 84,870.50 | 6,608.50 | -92.21% |
| 16,384 | all_changed | early | 84,870.50 | 85,526.75 | +0.77% |
| 16,384 | one_changed | middle | 84,870.50 | 6,360.00 | -92.51% |
| 16,384 | all_changed | middle | 84,870.50 | 77,574.75 | -8.60% |
| 16,384 | one_changed | late | 84,870.50 | 6,113.75 | -92.80% |
| 16,384 | all_changed | late | 84,870.50 | 69,694.75 | -17.88% |
| 16,384 | one_changed | replacement | 84,870.50 | 6,608.50 | -92.21% |
| 16,384 | all_changed | replacement | 84,870.50 | 85,526.75 | +0.77% |
| 524,288 | cold | none | 2,113,316.00 | 2,113,318.00 | +0.00% |
| 524,288 | unchanged | none | 2,113,316.00 | 4,110.00 | -99.81% |
| 524,288 | one_changed | early | 2,113,316.00 | 531,562.00 | -74.85% |
| 524,288 | all_changed | early | 2,113,316.00 | 2,113,974.00 | +0.03% |
| 524,288 | one_changed | middle | 2,113,316.00 | 531,992.00 | -74.83% |
| 524,288 | all_changed | middle | 2,113,316.00 | 2,115,694.00 | +0.11% |
| 524,288 | one_changed | late | 2,113,316.00 | 528,422.00 | -75.00% |
| 524,288 | all_changed | late | 2,113,316.00 | 2,101,414.00 | -0.56% |
| 524,288 | one_changed | replacement | 2,113,316.00 | 531,562.00 | -74.85% |
| 524,288 | all_changed | replacement | 2,113,316.00 | 2,113,974.00 | +0.03% |

B/file is median set allocation divided by file count (32 files for the two smaller sizes, four for the largest). Unicode input has a different UTF-8 byte count; the raw packet records both sizes. Every file is still read in every scenario. `one_changed` edits one file per set; `all_changed` edits every file. Early/middle/late edits preserve encoded length and avoid splitting surrogate pairs; replacement changes the whole text using characters of the same widths. The probe validates every decoded text and SHA-256 hash and verifies unchanged string identity.

The first draft measured only trailing-unit edits. Review prompted the broader matrix above. Some early/full replacements allocate slightly more: about 0.8% for the middle file size and at most about 0.1% for the largest. The no-prior path adds eight closure bytes per set. This is a measured tradeoff, not a universal allocation or latency win. A retained string is available only after the active module has parsed source; publication-only cache hits need not have such a string.

Validation: audited Release build has zero warnings/errors; all 328 .NET tests (232 compiler) pass, including BOM variants, malformed UTF-8 fallback, Unicode paths, same-size/same-mtime edits, deletion/locking, eviction and failures after a read block. Eight real native project/lifecycle/L1-L2-L3 tests pass. The 2,048-script, nine-case warm-versus-fresh matrix passes 18 responses, 81 module comparisons, 71 new PE/PDB comparisons and 32,256 scalar witnesses. A separate before/after dependency fixture matches exact PE/PDB/schema for 27 module pairs. No identity normalization is used.

Only scalar witnesses execute in the matrix; query bodies match fresh emission byte for byte. Gameplay execution remains covered by the native tests. No new time-to-Play, compile latency, frame rate, retained-state p95, peak RSS or unload claim follows. The original dependency and retained-state timing questions remain open.

[Raw samples, every binary/runtime/source manifest, all case summaries and quality bindings](source_capture_allocations_mac_2026-11-10.json).

```sh
dotnet build benchmarks/iteration/source_hash/SourceHash.csproj -c Release -m:2 --disable-build-servers
python3 benchmarks/iteration/source_capture_pairs.py --baseline /absolute/baseline/Ara.CompileServer.dll --candidate /absolute/candidate/Ara.CompileServer.dll --helper target/iteration-perf/source-hash-probe/bin/SourceHash/release/SourceHash.dll --dotnet-host /absolute/dotnet --output /absolute/new-output
```

The optional probe requires complete frozen compiler output closures, writes only in its new output directory, and accepts no timings. It is not a build prerequisite.

## Actual project-preparation counters

A separate four-process ABBA run of the real project-session CLI used eight modules × 128 component/query files, one edited source per module/request, five warmups and ten measured requests/process. Its startup hook counted all managed allocations between reply boundaries. Median allocation across twenty measured requests/arm was **707,275,088 → 700,242,272 bytes/request**, about **1.0% lower**. Exact PE/PDB/schema parity passed 360 module-pair comparisons. These numbers put the direct-method savings in context: Roslyn and the rest of preparation still dominate total allocation.

All clocks remain unaccepted: this was an instrumented counter diagnosis under external load, not an admitted latency experiment. Logger allocation is excluded, but the hook can perturb GC phase. The final request forces collect/finalize/collect with compiler state and the last reply rooted, so its time is deliberately invalid; its heap is neither peak RSS nor normal steady state. No retained-state p95 cause or fix is established. [Every request counter, raw unaccepted clock and frozen input binding](project_capture_cli_allocations_mac_2026-11-10.json).
