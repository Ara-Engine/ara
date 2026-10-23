# arabench v0 · codex:gpt-6-luna · after2

Started 2026-10-23T01:48:23Z on engine `09cc127+dirty`. **22/22 passed (100%)**, 700 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 5 | 5 | 31s | 143k/1k | ARA1106×2 | - |
| `scene-turn-and-raise` | assembly | pass | 2 | 2 | 15s | 113k/1k | - | - |
| `scene-group-barrels` | assembly | pass | 9 | 9 | 41s | 206k/2k | ARA1106 | - |
| `component-beacon-light` | components | pass | 4 | 4 | 23s | 101k/1k | - | - |
| `property-halve-lights` | components | pass | 3 | 3 | 18s | 171k/1k | - | - |
| `component-remove-rigidbody` | components | pass | 3 | 3 | 14s | 188k/1k | ARA1101 | - |
| `prefab-stack-crates` | prefabs | pass | 6 | 6 | 18s | 227k/1k | - | - |
| `prefab-tint-one-crate` | prefabs | pass | 7 | 7 | 35s | 156k/1k | - | - |
| `prefab-revert-keep-position` | prefabs | pass | 15 | 15 | 67s | 321k/2k | ARA1106 | - |
| `prefab-blue-bulb` | prefabs | pass | 6 | 6 | 30s | 280k/1k | - | - |
| `hierarchy-radio-to-bench` | hierarchy | pass | 11 | 11 | 48s | 277k/2k | ARA1101 ARA1105 | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 3 | 3 | 14s | 135k/1k | - | - |
| `refs-fix-broken` | references | pass | 12 | 12 | 59s | 315k/3k | ARA1111 ARA1302×3 ARA1401×2 | 2 |
| `merge-resolve-conflict` | merge | pass | 6 | 6 | 59s | 155k/1k | - | - |
| `diag-fix-typos` | diagnostics | pass | 6 | 6 | 37s | 167k/2k | ARA1101 | 1 |
| `schema-door-fields` | schema | pass | 9 | 9 | 28s | 131k/2k | ARA1204 | - |
| `undo-other-agent` | undo | pass | 3 | 3 | 21s | 164k/1k | - | - |
| `docs-cheatsheet` | docs | pass | 4 | 4 | 17s | 99k/1k | - | - |
| `batch-watchtower` | batch | pass | 8 | 8 | 32s | 183k/2k | ARA1106×2 ARA1602 | - |
| `dryrun-clear-ruins` | dry-run | pass | 6 | 6 | 25s | 136k/1k | - | - |
| `multi-storage-room` | multi-step | pass | 21 | 21 | 41s | 262k/2k | ARA1106×3 | - |
| `multi-qa-fixes` | multi-step | pass | 6 | 6 | 26s | 154k/2k | - | - |
