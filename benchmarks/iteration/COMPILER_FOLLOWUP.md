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
