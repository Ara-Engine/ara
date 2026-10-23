# arabench v0 · claude:sonnet-5.5 · after

Started 2026-10-23T01:36:45Z on engine `d3fa6b3+dirty`. **22/22 passed (100%)**, 465 s of agent time, $1.80.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 6 | 6 | 17s | 195k/2k | ARA1602 | - |
| `scene-turn-and-raise` | assembly | pass | 4 | 4 | 17s | 167k/1k | - | - |
| `scene-group-barrels` | assembly | pass | 8 | 8 | 22s | 246k/2k | ARA1602 | - |
| `component-beacon-light` | components | pass | 4 | 4 | 13s | 132k/1k | - | - |
| `property-halve-lights` | components | pass | 5 | 5 | 14s | 134k/2k | - | - |
| `component-remove-rigidbody` | components | pass | 3 | 3 | 10s | 129k/1k | - | - |
| `prefab-stack-crates` | prefabs | pass | 8 | 8 | 25s | 284k/2k | ARA1301 ARA1602 | - |
| `prefab-tint-one-crate` | prefabs | pass | 4 | 4 | 13s | 168k/1k | - | - |
| `prefab-revert-keep-position` | prefabs | pass | 7 | 7 | 19s | 205k/2k | - | - |
| `prefab-blue-bulb` | prefabs | pass | 5 | 5 | 14s | 170k/1k | - | - |
| `hierarchy-radio-to-bench` | hierarchy | pass | 4 | 4 | 13s | 133k/1k | - | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 7 | 7 | 21s | 169k/1k | - | - |
| `refs-fix-broken` | references | pass | 7 | 7 | 49s | 198k/2k | ARA1302 ARA1401 | 3 |
| `merge-resolve-conflict` | merge | pass | 5 | 5 | 50s | 146k/1k | - | 3 |
| `diag-fix-typos` | diagnostics | pass | 4 | 4 | 47s | 127k/1k | - | 2 |
| `schema-door-fields` | schema | pass | 5 | 5 | 14s | 167k/1k | - | - |
| `undo-other-agent` | undo | pass | 4 | 4 | 11s | 131k/1k | - | - |
| `docs-cheatsheet` | docs | pass | 6 | 6 | 13s | 125k/2k | - | - |
| `batch-watchtower` | batch | pass | 6 | 6 | 24s | 211k/3k | ARA1301 ARA1602 | - |
| `dryrun-clear-ruins` | dry-run | pass | 5 | 5 | 20s | 168k/2k | - | - |
| `multi-storage-room` | multi-step | pass | 8 | 8 | 20s | 218k/2k | ARA1106 ARA1204 | - |
| `multi-qa-fixes` | multi-step | pass | 8 | 8 | 19s | 206k/2k | - | - |
