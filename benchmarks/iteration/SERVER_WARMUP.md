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

## Dedicated threads, a retrying framework set, and a core-count gate (review round)
- The warm-up no longer blocks a thread-pool thread on two others: each part runs on its own below-normal background thread and the warm-up is a `Task` that is never awaited by the compile path. A failed part is reported once on stderr (`[ara] compile warm-up failed ...`).
- The shared framework reference set is built with `LazyThreadSafetyMode.PublicationOnly`, so a failed read (a scanner holding a file) is retried by the next caller instead of failing every compile in the process (test: a factory that fails once).
- Measured with the machine limited to about two cores (`DOTNET_PROCESSOR_COUNT=2` plus 12 busy-loop processes; macOS has no CPU affinity, so this is an emulation), 20 fresh processes per arm, launch to first tick: no warm-up 1389 median / 1588 p95 (max 2017) against 1345 / 2357 (max 3480) with the warm-up. The median barely moves and the tail gets worse, so the warm-up now runs only with at least 8 logical cores.
- On the whole machine (15 cores), same method: no warm-up 786 / 817 (min 770) against 665 / 690 (min 625), i.e. -121 ms median and a better p95. (An earlier note measured the two-thread warm-up against the one-thread one: 700 -> 654.)
Raw rows: `server_warmup_cores_mac_2026-11-11.json`.

