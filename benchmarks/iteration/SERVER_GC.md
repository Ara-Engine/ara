# Compile server GC mode (Mac, 2026-11-11)

`reload_timing.cs --trace` (#54) showed the compile server's GC on the critical path: an L3 compile of 122 ms had about 10 gen0, 3 gen1 and 1 gen2 collection and 44 ms of GC pause; the initial compile 135 ms (29 gen0, 11 gen1, 3 gen2). A compile allocates 100-160 MB in ~100 ms, and with concurrent (background) GC enabled the runtime budgets gen0 far smaller than the cache-sized budget it uses otherwise, so those allocations trigger collection after collection. The same compile with `System.GC.Concurrent=false` has 1-2 collections (the setting is in `runtimeconfig.json`, so no spawn site needs an environment variable). `DOTNET_GCgen0size=0x10000000` gave the same result in the experiment but can only be set through the environment.

Cost: the gen0 budget grows with the setting, so peak server memory rises: peak RSS (sampled with `ps`, 4 runs per arm) 238-253 MB -> 310-318 MB. Blocking gen2 collections now happen in the compile server (never in the game process); the server's gen2 count per compile is 0-1.

20 fresh processes per arm, mirrored order, 45 edits per process (300 per level per arm). Median / p95, ms, save -> visible, and launch to first tick:

| | concurrent GC (before) | non-concurrent (after) |
| --- | ---: | ---: |
| launch to first tick | 837 / 1117 | 807 / 949 |
| L1 | 23.7 / 51.4 | 22.5 / 49.2 |
| L2 | 32.8 / 49.9 | 28.7 / 50.1 |
| L3 | 128.3 / 156.7 | 108.3 / 130.3 |
| L3 emit (server) | 94 | 75 |

Raw rows: `server_nonconcurrent_gc_mac_2026-11-11.json`. Not measured on Windows or Linux: concurrent GC's gen0 budget rule is the runtime's, not the OS's, so the effect should carry over, but the Windows L1 tail (emit p95 71 ms against a 5 ms median) needs `--trace` on that machine to say whether GC is what it is.
