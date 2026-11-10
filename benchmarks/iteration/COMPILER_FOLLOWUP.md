# Compiler follow-up (Mac, 2026-11-10)

`compiler_incremental_parse_mac_2026-11-10.json` completes a fresh ABBA comparison of the archived incremental-parse optimization: four real ScriptHosts, 60 edits each, 20 per reload level. All 240 edits produced the intended level and visible output, with zero leaked generations. Full raw edit rows, SDK/executable hashes and runtime-image hashes are retained. CoreCLR is 10.0.14 in both arms. This is a historical optimization comparison, not current-main or Unity performance.

| Reload | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
| --- | ---: | ---: |
| L1 | 126.297 / 160.651 | 117.331 / 153.310 |
| L2 | 135.570 / 169.583 | 122.927 / 144.364 |
| L3 | 216.197 / 238.131 | 206.898 / 227.119 |

These pooled figures use 40 samples per arm/level. The packet also retains process summaries; requests in one process are correlated. No confidence interval or universal performance claim follows from this small campaign.

Reproduce with complete archived SDK project roots and an executable `reload_bench`:

```sh
python3 benchmarks/iteration/reload_pairs.py --exe /absolute/reload_bench --baseline-bin /absolute/baseline-bin --candidate-bin /absolute/candidate-bin --dotnet-root /absolute/dotnet --output /absolute/new-results --edits 60
```

The compiler-only counterpart is `compiler_pairs.py`: `--baseline` and `--candidate` take server DLL paths, `--dotnet-host` pins the executable, and `--modules`, `--files`, `--samples`, `--warmups` select a finite fixture. It runs ABBA at one absolute project path and requires exact PE/PDB/schema equality. `--dependency-fanout` edits a shared constant and rebuilds unchanged consumers. `--quality-only` runs two arms and accepts no timings.

`compiler_scale.py` now has bounded response waits, concurrent stderr draining, exact module/schema counts, every raw request duration, explicit warmup count and an optional fixed fixture path. The CPU guard retains sampled activity and rejects competing compilers; it cannot observe every short-lived task or prove absence of other background load.

Dependency and retained-state attempts overlapped Renderlab builds/tests and remain unaccepted. The historical retained-state p95 increase is still open; it is not relabelled a win. A small fixed-dependency quality fixture passed exact output parity, but its durations are not performance evidence. Original bundles produced different PE/PDB identities, so any fixed-dependency comparison must be labelled compiler-only.

## Source fingerprint buffers

The new production change hashes the same captured UTF-8 text using up to 1 KiB of stack space, or a rented buffer for larger files, and a stack SHA-256 digest. A `finally` returns rented memory. The returned hash string owns its bytes. There is no file-read skipping or metadata trust. `Ara.Host` links the same implementation.

Four fresh processes ABBA measured direct production calls with per-thread allocation counters, 32 warmups/input and 20 measured batches/input/process:

| UTF-8 input | Baseline bytes/call | Candidate bytes/call |
| --- | ---: | ---: |
| 384 B ASCII | 616 | 152 |
| 768 B Unicode | 1,000 | 152 |
| 1,024 B boundary | 1,256 | 152 |
| 1,025 B boundary | 1,264 | 152 |
| 16 KiB | 16,616 | 152 |
| 512 KiB | 525,336 | 152 |

These are median raw batch allocations, including the returned hexadecimal string. Pool cold-rent costs are excluded. Raw timings have no independent host admission and **must not be read as compile-time or p95 improvements**. [Raw allocation packet](source_hash_allocations_mac_2026-11-10.json) retains every batch and both binary manifests.

Validation: audited Release build with no warnings/errors, all 316 .NET tests (220 compiler), seven real native project/lifecycle/reload tests, 116-package formatting, and exact PE/PDB/schema equality across 96 module pairs from a 1,024-script fixture. New hash tests cover stack/pool boundaries, random UTF-16 including malformed surrogates, and concurrent large files.

Reproduce the optional direct-call allocation helper using only .NET 10:

```sh
dotnet build benchmarks/iteration/source_hash/SourceHash.csproj -c Release --disable-build-servers
dotnet target/iteration-perf/source-hash-probe/bin/SourceHash/release/SourceHash.dll /absolute/immutable-sdk/Ara.CompileServer.dll
```

`plot_followup.py --output /absolute/graphs` regenerates the publication PNGs (optional matplotlib dependency; never a build prerequisite).

## SDK fingerprint streams

The next production slice replaces whole-assembly byte arrays with standard `SHA256.HashData(Stream)` for tool/runtime/generator/editor identities. Every byte is still read on every identity computation. File sharing remains read-only and `using` closes each handle. There is no metadata cache or custom decoder. Streaming may issue more reads; allocation savings alone do not establish a latency gain.

Twelve fresh direct-call processes (ABBA per four-file set), four warmups plus twenty measured sets/process, all digests equal:

| Four-file set | Baseline allocated bytes | Candidate allocated bytes |
| --- | ---: | ---: |
| Actual compiler/runtime/generator/editor DLLs | 721,672 | 1,992 |
| Four 64 KiB files | 263,432 | 1,992 |
| Four 1 MiB files | 4,195,816 | 1,992 |

These medians count reads, hash work, result arrays and strings on the current thread. No clocks are admitted. The [raw SDK packet](sdk_hash_allocations_mac_2026-11-10.json) retains every sample, exact input hashes, production DLL identity and source bindings. The optional helper also accepts `--sdk-files /absolute/files.json` after the server path; baseline DLLs without `HashFile` call the original production `Hash(File.ReadAllBytes)` path. Input writes and reference digests stay outside the counter.

Audited Release build, all 318 .NET tests (222 compiler), and six native publication/lifecycle tests pass. SDK hashing tests cover empty/small/page-boundary/large files, Unicode paths, exclusive sharing errors, missing/directory errors, immediate handle release, and actual installed SDK images.

[Rejected campaign ledger](compiler_followup_rejections_mac_2026-11-10.json) preserves each failed admission, its first observer error, raw unaccepted request durations and report hashes. The original 2026-11-09 one-way dependency pair remains historical evidence; no rejected follow-up fills in its missing reverse arm.

## Retained-state GC diagnosis

A diagnostic startup hook recorded real archived CLI reply boundaries for four fresh processes ABBA, 32 modules × 32 scripts, five warmups plus ten measured requests/process. Original compiler DLL hashes match the historical 8-entry/all-active-state comparison. Both arms use fixed baseline common dependencies. Every emitted PE/PDB/schema matches the corresponding revision in the other processes (1,440 module-pair comparisons). The hook preserves the original CLI scheduling policy and JSON replies.

| Counter | Eight-entry cache | All 32 active modules |
| --- | ---: | ---: |
| Median allocation/request | 814,471,476 B | 759,204,400 B |
| Final live managed heap (median of two processes) | 73,812,280 B | 166,058,396 B |
| Gen2 collections across 20 measured requests | 206 | 84 |

Keeping active state retains about 92.2 MB more managed heap while reducing allocation about 6.8%. This identifies a real memory/collection tradeoff; it **does not establish a p95 cause or fix**. GC pause durations and all other clocks are unaccepted because no independent host admission was applied. The logger can shift GC phase. A final collect/finalize/collect deliberately invalidates the last request's timing; its live-heap result includes active state and the final reply, and is not peak RSS or ordinary steady state. CoreCLR here is 10.0.7, separate from the 10.0.14 live-edit/source-hash campaigns.

[All counters, raw unaccepted durations, immutable SDK/hook hashes and exact output hashes](compiler_retained_gc_mac_2026-11-10.json) are retained. The next timing-dependent step stays open until a quiet Mac window: determine whether the historical six-sample p95 increase repeats. Allocation work can proceed separately; these source-hash/SDK-stream savings are small parts of total compilation allocation and do not prove a whole-compiler speedup.

Reproduce the diagnostic independently of timing campaigns:

```sh
dotnet build benchmarks/iteration/project_gc/Hook.csproj -c Release --disable-build-servers
python3 benchmarks/iteration/retained_gc.py --baseline /absolute/baseline/Ara.CompileServer.dll --candidate /absolute/candidate/Ara.CompileServer.dll --hook "$PWD/target/iteration-perf/project-gc-hook/bin/Hook/release/Hook.dll" --dotnet-host /absolute/dotnet --output /absolute/fresh-diagnostic
```

The checked-in portable controller and hook also passed a fresh four-process tiny quality run; every response had exactly one counter row and exact PE/PDB/schema parity. This is a diagnostic instrument, not part of the engine or a gameplay state store.
