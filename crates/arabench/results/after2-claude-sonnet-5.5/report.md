# arabench v0 · claude:sonnet-5.5 · after2

Started 2026-10-23T01:48:01Z on engine `09cc127+dirty`. **22/22 passed (100%)**, 604 s of agent time, $1.81.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 5 | 5 | 15s | 160k/1k | - | - |
| `scene-turn-and-raise` | assembly | pass | 4 | 4 | 15s | 166k/1k | - | - |
| `scene-group-barrels` | assembly | pass | 5 | 5 | 18s | 170k/2k | - | - |
| `component-beacon-light` | components | pass | 4 | 4 | 15s | 132k/1k | - | - |
| `property-halve-lights` | components | pass | 8 | 8 | 27s | 196k/2k | ARA1111 | - |
| `component-remove-rigidbody` | components | pass | 3 | 3 | 17s | 131k/1k | - | - |
| `prefab-stack-crates` | prefabs | pass | 7 | 7 | 20s | 210k/2k | ARA1302 | - |
| `prefab-tint-one-crate` | prefabs | pass | 4 | 4 | 18s | 169k/1k | - | - |
| `prefab-revert-keep-position` | prefabs | pass | 8 | 8 | 24s | 278k/2k | - | - |
| `prefab-blue-bulb` | prefabs | pass | 6 | 6 | 15s | 171k/1k | - | - |
| `hierarchy-radio-to-bench` | hierarchy | pass | 8 | 8 | 23s | 247k/2k | - | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 6 | 6 | 17s | 207k/1k | - | - |
| `refs-fix-broken` | references | pass | 5 | 5 | 79s | 144k/2k | - | 2 |
| `merge-resolve-conflict` | merge | pass | 5 | 5 | 97s | 146k/2k | - | 2 |
| `diag-fix-typos` | diagnostics | pass | 2 | 2 | 60s | 84k/1k | - | 1 |
| `schema-door-fields` | schema | pass | 6 | 6 | 18s | 173k/1k | - | - |
| `undo-other-agent` | undo | pass | 4 | 4 | 16s | 129k/1k | - | - |
| `docs-cheatsheet` | docs | pass | 6 | 6 | 23s | 139k/2k | - | - |
| `batch-watchtower` | batch | pass | 6 | 6 | 28s | 208k/2k | ARA1302 | - |
| `dryrun-clear-ruins` | dry-run | pass | 5 | 5 | 13s | 168k/1k | - | - |
| `multi-storage-room` | multi-step | pass | 10 | 10 | 24s | 225k/3k | ARA1101 | - |
| `multi-qa-fixes` | multi-step | pass | 7 | 7 | 24s | 208k/2k | - | - |
