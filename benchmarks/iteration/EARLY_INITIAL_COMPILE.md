# First compile before the file watchers (Mac, 2026-11-11)

What the ~1.0 s from launch to first tick was made of, from timestamps in a throwaway server build (ms since the server process started; the host itself starts in ~30 ms):

| step | ms |
| --- | ---: |
| server `Main` entered | 35 (200+ on a cold disk cache) |
| pipe connected, coordinator created, Hello and Watch received | 100 |
| `ModuleSession` created (framework references, generator driver) | +57 |
| module `FileSystemWatcher` started | **+190** |
| reference-assembly `FileSystemWatcher` started | **+195** |
| first compile starts | +22 |
| parse / generate / emit (cold) | 39 / 196 / 305 |
| host: pre-JIT, register, first tick | ~30 |

`Coordinator.Watch` created both watchers inside its lock before it queued the first compile, and a `FileSystemWatcher` takes about 200 ms to start on macOS (the FSEvents stream). That was ~390 ms, 40% of the launch, spent before any compile work (and once per module on the server's message loop, so a project of N modules waited N × 200 ms). Now `Watch` queues the first compile, then starts the watchers on a background task. A save made between the request and the watcher going live would be missed by both the first compile and the watcher, so once the watcher is live any source file modified since the request is compiled as a change (rare; otherwise nothing is compiled). `Dispose` waits for pending starts.

Launch to first tick (host start + initial compile in `reload_bench`), 20 fresh processes per arm, mirrored order:

| | median | p95 | min |
| --- | ---: | ---: | ---: |
| before | 1129 | 1167 | 879 |
| after | 775 | 789 | 759 |

Next largest piece: the cold first generator run and emit (196 + 305 ms), which is Roslyn's first-use cost after ReadyToRun (type loading, fixups, framework reference metadata), then the 100 ms the server needs before it can receive the first `Watch`. Raw rows: `early_initial_compile_mac_2026-11-11.json`.
