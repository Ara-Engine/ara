# arabench v0 · codex:gpt-6-luna · after

Started 2026-10-23T01:37:01Z on engine `d3fa6b3+dirty`. **22/22 passed (100%)**, 508 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 12 | 12 | 28s | 242k/2k | - | - |
| `scene-turn-and-raise` | assembly | pass | 2 | 2 | 11s | 103k/1k | - | - |
| `scene-group-barrels` | assembly | pass | 4 | 4 | 17s | 183k/1k | - | - |
| `component-beacon-light` | components | pass | 9 | 9 | 23s | 175k/1k | ARA1101 | - |
| `property-halve-lights` | components | pass | 4 | 4 | 16s | 205k/1k | - | - |
| `component-remove-rigidbody` | components | pass | 3 | 3 | 14s | 159k/1k | - | - |
| `prefab-stack-crates` | prefabs | pass | 6 | 6 | 24s | 173k/1k | - | - |
| `prefab-tint-one-crate` | prefabs | pass | 10 | 10 | 27s | 154k/2k | - | - |
| `prefab-revert-keep-position` | prefabs | pass | 8 | 8 | 30s | 183k/2k | - | - |
| `prefab-blue-bulb` | prefabs | pass | 4 | 4 | 19s | 194k/1k | - | - |
| `hierarchy-radio-to-bench` | hierarchy | pass | 2 | 2 | 10s | 104k/1k | - | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 4 | 4 | 17s | 137k/1k | - | - |
| `refs-fix-broken` | references | pass | 6 | 6 | 29s | 153k/2k | ARA1111 ARA1302 ARA1401 | 2 |
| `merge-resolve-conflict` | merge | pass | 9 | 9 | 31s | 211k/2k | - | 1 |
| `diag-fix-typos` | diagnostics | pass | 15 | 15 | 38s | 246k/3k | ARA1101 | - |
| `schema-door-fields` | schema | pass | 6 | 6 | 21s | 227k/1k | ARA1101 ARA1105 | - |
| `undo-other-agent` | undo | pass | 3 | 3 | 19s | 222k/1k | - | - |
| `docs-cheatsheet` | docs | pass | 5 | 5 | 21s | 119k/1k | - | - |
| `batch-watchtower` | batch | pass | 8 | 8 | 22s | 273k/2k | ARA1101×2 | - |
| `dryrun-clear-ruins` | dry-run | pass | 7 | 7 | 26s | 157k/2k | - | - |
| `multi-storage-room` | multi-step | pass | 17 | 17 | 42s | 377k/3k | ARA1101×2 ARA1105×2 ARA1106 ARA1204 | - |
| `multi-qa-fixes` | multi-step | pass | 6 | 6 | 25s | 150k/2k | ARA1111 | - |
