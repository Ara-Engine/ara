# Roslyn warm-up at server start (Mac, 2026-11-11)

After the first compile moved ahead of the file watchers, the server still idles ~100 ms between starting and receiving the host's first `Watch` (pipe connect, Hello, framework reference metadata) while the first compile then pays Roslyn's cold costs (generate 196 ms, emit 305 ms: type loading, reference metadata, generator driver). `Program` now starts `ModuleCompiler.WarmUp()` on a thread-pool thread before it connects: one component and one system through the generator and a debug-IL emit (the framework references are the same static ones the real compile uses). Failure is ignored; the warm-up changes no module state.

Launch to first tick (host start + initial compile in `reload_bench`), 20 fresh processes per arm, mirrored order, on top of the first-compile-before-watchers change:

| | median | p95 | min |
| --- | ---: | ---: | ---: |
| before | 788 | 897 | 774 |
| after | 710 | 856 | 681 |

On the old ordering, where the watchers gave the warm-up a ~400 ms head start, the same step took 890 → 755 ms (3 runs each, not a campaign); with the first compile queued early the head start is ~100 ms and the gain is the 78 ms above. Measured on a 15-core Mac only: the warm-up runs beside the real compile for a few hundred ms, so on a machine with few cores it may help less (or contend); the p95 difference is within noise. Per-edit levels are unchanged (L1 49 → 53, L2 52 → 55, L3 244 → 242 ms over 40 edits per arm, run right after launch). Raw rows: `server_warmup_mac_2026-11-11.json`.

## Two threads (generator and emit)
The first warm-up ran the generator and then the emit on one thread, so it contended with the real compile's generator phase on the same cold code, and the emit paths were only warm if the warm-up got there first. The real compile runs the generator first (~230 ms cold) and emits after, so the warm-up now uses two threads at once: one runs the generator over a small component and system, the other binds and emits a plain debug-IL module with a PDB (a struct, a loop, LINQ with lambdas, `Math`). By the time the real emit starts its paths are warm.

20 fresh processes per arm, mirrored order, launch to first tick on the Mac (on top of #37 and #38): median 700 → 654 ms, p95 925 → 881, min 665 → 616. Per-edit levels unchanged (they are measured right after launch; L3 differs only because the two builds carry different L3 scoping). Same caveat as #38: measured on a 15-core Mac; the two threads run beside the real compile for a few hundred ms, so a machine with few cores may gain less. Raw rows: `server_warmup_split_mac_2026-11-11.json`.

