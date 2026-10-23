# arabench v0 · claude:sonnet-5.5 · baseline

Started 2026-10-23T01:04:55Z on engine `642e6ad+dirty`. **22/22 passed (100%)**, 933 s of agent time, $2.13.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 8 | 8 | 27s | 238k/2k | ARA1602 ARA1803 | - |
| `scene-turn-and-raise` | assembly | pass | 7 | 7 | 35s | 236k/2k | - | - |
| `scene-group-barrels` | assembly | pass | 8 | 8 | 52s | 242k/2k | ARA1506 | - |
| `component-beacon-light` | components | pass | 8 | 8 | 28s | 237k/2k | ARA1803 | - |
| `property-halve-lights` | components | pass | 10 | 10 | 54s | 293k/2k | - | - |
| `component-remove-rigidbody` | components | pass | 7 | 7 | 32s | 201k/2k | - | - |
| `prefab-stack-crates` | prefabs | pass | 10 | 10 | 36s | 211k/2k | - | - |
| `prefab-tint-one-crate` | prefabs | pass | 9 | 9 | 42s | 246k/2k | - | - |
| `prefab-revert-keep-position` | prefabs | pass | 9 | 9 | 25s | 207k/2k | - | - |
| `prefab-blue-bulb` | prefabs | pass | 11 | 11 | 34s | 284k/2k | ARA1106 | - |
| `hierarchy-radio-to-bench` | hierarchy | pass | 9 | 9 | 29s | 249k/2k | - | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 10 | 10 | 37s | 206k/2k | - | - |
| `refs-fix-broken` | references | pass | 3 | 3 | 68s | 114k/1k | - | 2 |
| `merge-resolve-conflict` | merge | pass | 6 | 6 | 91s | 201k/2k | - | 2 |
| `diag-fix-typos` | diagnostics | pass | 4 | 4 | 100s | 141k/1k | - | 2 |
| `schema-door-fields` | schema | pass | 10 | 10 | 31s | 283k/2k | - | - |
| `undo-other-agent` | undo | pass | 8 | 8 | 34s | 233k/2k | - | - |
| `docs-cheatsheet` | docs | pass | 7 | 7 | 33s | 200k/2k | - | - |
| `batch-watchtower` | batch | pass | 8 | 8 | 23s | 207k/2k | ARA1803 | - |
| `dryrun-clear-ruins` | dry-run | pass | 9 | 9 | 48s | 276k/2k | - | - |
| `multi-storage-room` | multi-step | pass | 11 | 11 | 36s | 260k/3k | - | - |
| `multi-qa-fixes` | multi-step | pass | 11 | 11 | 38s | 317k/2k | ARA1808 | - |
