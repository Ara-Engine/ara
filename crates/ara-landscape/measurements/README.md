# 4096 m landscape measurement

RTX 4070 SUPER, 2560x1440, 1,390,348 input foliage instances. Fixed tick 120,
seed 42, eight warmup frames and 32 measured frames per lighting state.
16 terrain tiles were read asynchronously and published into a 256-page detail
pool, at most one 13,084-byte tile upload per frame. Shader/driver provenance and
actual emitted terrain/foliage LOD counts are in [gpu-timings.json](gpu-timings.json).

GPU timestamps in milliseconds:

| Pass | Noon p50 | Noon p95 | Dusk p50 | Dusk p95 |
|---|---:|---:|---:|---:|
| Reset draw arguments | 0.004 | 0.013 | 0.005 | 0.008 |
| Terrain LOD selection | 0.002 | 0.002 | 0.002 | 0.002 |
| Terrain culling | 0.004 | 0.005 | 0.004 | 0.004 |
| Terrain depth | 0.041 | 0.042 | 0.042 | 0.045 |
| Terrain shadows | 0.071 | 0.072 | 0.044 | 0.045 |
| Terrain colour | 0.215 | 0.479 | 0.216 | 0.685 |
| Terrain HZB | 0.010 | 0.011 | 0.010 | 0.011 |
| Foliage culling | 0.183 | 0.201 | 0.184 | 0.199 |
| Foliage depth | 0.036 | 0.036 | 0.036 | 0.041 |
| Foliage shadows | 0.031 | 0.036 | 0.034 | 0.037 |
| Foliage colour | 0.057 | 0.058 | 0.057 | 0.061 |
| Sky | 0.288 | 0.566 | 0.039 | 0.362 |
| Tile publication (16 samples) | 0.008 | 0.015 | — | — |

CPU frame preparation/recording/submission p50/p95 was 0.913/3.703 ms at noon
and 0.907/1.955 ms at dusk, excluding the diagnostic completion wait. Cache reads
and decoding run outside the frame thread. This is a serialized diagnostic run
on a shared machine, not an end-to-end editor FPS guarantee; p99 values and
outliers are preserved in the JSON. The scene uses procedural stand-in assets.

All 14 focused tests, cargo check, clippy with `-D warnings`, and rustfmt check
passed after merging main. Native tests include Jolt seams/holes, GPU goldens,
identical repeat frames, streamed edits and authored alpha-tested mesh LODs.
Vulkan validation/performance diagnostics were clean.
