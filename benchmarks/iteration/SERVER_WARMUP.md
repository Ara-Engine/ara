# Roslyn warm-up at server start (Mac, 2026-11-11)

After the first compile moved ahead of the file watchers, the server still idles ~100 ms between starting and receiving the host's first `Watch` (pipe connect, Hello, framework reference metadata) while the first compile then pays Roslyn's cold costs (generate 196 ms, emit 305 ms: type loading, reference metadata, generator driver). `Program` now starts `ModuleCompiler.WarmUp()` on a thread-pool thread before it connects: one component and one system through the generator and a debug-IL emit (the framework references are the same static ones the real compile uses). Failure is ignored; the warm-up changes no module state.

Launch to first tick (host start + initial compile in `reload_bench`), 20 fresh processes per arm, mirrored order, on top of the first-compile-before-watchers change:

| | median | p95 | min |
| --- | ---: | ---: | ---: |
| before | 788 | 897 | 774 |
| after | 710 | 856 | 681 |

On the old ordering, where the watchers gave the warm-up a ~400 ms head start, the same step took 890 → 755 ms (3 runs each, not a campaign); with the first compile queued early the head start is ~100 ms and the gain is the 78 ms above. Measured on a 15-core Mac only: the warm-up runs beside the real compile for a few hundred ms, so on a machine with few cores it may help less (or contend); the p95 difference is within noise. Per-edit levels are unchanged (L1 49 → 53, L2 52 → 55, L3 244 → 242 ms over 40 edits per arm, run right after launch). Raw rows: `server_warmup_mac_2026-11-11.json`.
