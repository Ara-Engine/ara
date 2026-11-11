# C# host reload measurements

## Measurements (`reload_bench`, 7800X3D shared with other agents' builds)
5.2k-line module, 20k entities, 60 Hz; edits rotate L1, L2 (a new method), L3 (a signature
change). v1 = `218d449` measured with the same bench (its additive edits take L3). Two
interleaved rounds; system CPU busy during each run in brackets. p50 / p95, ms:
| | v1 [48%, 28%] | v2 [35%, 39%] |
|---|---|---|
| L3 save→visible | 313 / 386, 265 / 334 | 282 / 299, 285 / 334 |
| additive edit save→visible | 305 / 391, 283 / 366 (L3) | 35 / 54, 41 / 56 (L2) |
| L1 save→visible | 25 / 32, 20 / 36 | 19 / 51, 19 / 34 |
| L3 emit | 109 / 180, 88 / 155 | 99 / 121, 106 / 116 |
| L3 load (AMSI) | 143 / 178, 136 / 144 | 134 / 144, 137 / 162 |
| L3 generators | 13 / 33, 11 / 28 | 18 / 47, 16 / 41 |
| L3 rest: detect, parse, transfer, pre-JIT, register, boundary, first tick (p50 sum) | 33, 25 | 20, 24 |
The L3 floor here is emit (~100 ms) plus the AMSI scan (~140 ms). Earlier v1 runs at 61–71%
CPU measured L3 427 / 614. Tick p99 2.5 ms during reloads; no leaked generations.

## Compile server ReadyToRun (Mac, 2026-11-11)
Save → visible on the Mac, 5.2k-line module (88 files), 20k entities, 60 Hz, `reload_bench`, other agents'
builds running (load 4-7). Roslyn ships IL only, so the old no-tiering server JIT-optimized every Roslyn method
on first use: start + initial compile spent 861 ms in the generator and 606 ms in emit, against ~6 ms and
~200 ms once warm. `dotnet build` of `Ara.CompileServer` now publishes with ReadyToRun into `obj/` and copies the
precompiled assemblies over the output (`-p:AraReadyToRun=false` restores the old build and no-tiering config).
The server tiers normally with PGO off and `CallCountingDelayMs=0`.

20 fresh processes per variant in mirrored order, 15 edits per level per process. Median / p95, ms:
| | IL, no tiering | ReadyToRun + tiering | + `CallCountingDelayMs=0` (shipped) |
|---|---|---|---|
| start + initial compile | 2262 / 2425 | 1009 / 1187 | 1068 / 1210 |
| first L1 edit after start | 149 / 170 | 73 / 86 | 66 / 73 |
| L1 save→visible | 38 / 139 | 40 / 68 | 37 / 63 |
| L2 save→visible | 139 / 166 | 148 / 189 | 138 / 161 |
| L3 save→visible | 234 / 261 | 250 / 300 | 233 / 261 |
A 90-edit run (first versus last 30 edits per level) shows the delay knob matters only early: late L3 is
236 ms against 238 ms for the old server. Raw rows: `benchmarks/iteration/compile_server_r2r_mac_2026-11-11.json`
(`startup_pairs.py` runs it). Edit phases at steady state, L1: ~12 ms in the server and ~8 ms waiting for the tick
boundary; L2: 90–120 ms of 145 is the full-module analyzer run (emit is 3 ms); L3: 150–290 ms is the full emit.
The package grows ~32 MB (R2R images are ~3x the IL). `FileSystemWatcher` on macOS delivers events 11 ms
(median) after the write, a third of an L1 save→visible.

## File-watcher latency per OS
`dotnet run benchmarks/iteration/watch_latency.cs` times how late `FileSystemWatcher` (the compile server's settings)
reports an in-place save and an atomic save (write a temp file, rename it over the source), median / p95 / max in ms.
Mac (FSEvents): 11.3 / 12.4 ms in place, 11.7 / 12.5 atomic. Run it on Windows and Linux to see whether their watchers
add a fixed delay (none is expected: ReadDirectoryChangesW and inotify report at once).
