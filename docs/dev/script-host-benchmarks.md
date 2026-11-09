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
