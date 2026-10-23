# arabench v0 · codex:gpt-6-luna · baseline

Started 2026-10-23T01:05:15Z on engine `642e6ad+dirty`. **20/22 passed (91%)**, 758 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | pass | 11 | 11 | 48s | 394k/3k | ARA1602 | - |
| `scene-turn-and-raise` | assembly | pass | 11 | 11 | 22s | 260k/1k | ARA1101 ARA1105 ARA1602×2 | - |
| `scene-group-barrels` | assembly | pass | 17 | 17 | 53s | 320k/2k | ARA1602×2 | - |
| `component-beacon-light` | components | pass | 14 | 14 | 43s | 349k/3k | ARA1102 ARA1602×3 | - |
| `property-halve-lights` | components | pass | 13 | 13 | 41s | 295k/2k | ARA1602×2 | - |
| `component-remove-rigidbody` | components | FAIL | 6 | 6 | 26s | 129k/1k | - | - |
| `prefab-stack-crates` | prefabs | pass | 12 | 12 | 41s | 293k/2k | ARA1602 | - |
| `prefab-tint-one-crate` | prefabs | pass | 12 | 12 | 20s | 262k/1k | ARA1204 | - |
| `prefab-revert-keep-position` | prefabs | pass | 12 | 12 | 24s | 336k/2k | - | - |
| `prefab-blue-bulb` | prefabs | pass | 9 | 9 | 40s | 222k/2k | ARA1602×2 | - |
| `hierarchy-radio-to-bench` | hierarchy | FAIL | 9 | 9 | 39s | 240k/2k | ARA1602 ARA1808 | - |
| `hierarchy-order-waypoints` | hierarchy | pass | 8 | 8 | 31s | 192k/1k | ARA1602 | - |
| `refs-fix-broken` | references | pass | 10 | 10 | 46s | 243k/2k | ARA1111 ARA1302 ARA1401 ARA1602 | 2 |
| `merge-resolve-conflict` | merge | pass | 13 | 13 | 61s | 282k/3k | ARA1001×3 ARA1209 | 3 |
| `diag-fix-typos` | diagnostics | pass | 4 | 4 | 21s | 96k/1k | ARA1101 | 2 |
| `schema-door-fields` | schema | pass | 14 | 14 | 54s | 331k/2k | ARA1602 ARA1808×2 | - |
| `undo-other-agent` | undo | pass | 3 | 3 | 14s | 73k/1k | - | - |
| `docs-cheatsheet` | docs | pass | 4 | 4 | 18s | 99k/1k | ARA1103 | - |
| `batch-watchtower` | batch | pass | 7 | 7 | 27s | 268k/2k | - | - |
| `dryrun-clear-ruins` | dry-run | pass | 7 | 7 | 22s | 247k/1k | - | - |
| `multi-storage-room` | multi-step | pass | 14 | 14 | 32s | 333k/3k | ARA1106 ARA1204 | - |
| `multi-qa-fixes` | multi-step | pass | 24 | 24 | 34s | 279k/2k | ARA1506 ARA1808×3 | - |

## Failures

### `component-remove-rigidbody`

- changed through commands with a `why`: without a why or from a file edit: tx_sdy6cp70 (component.remove)

### `hierarchy-radio-to-bench`

- Radio local position: got (0, 1.5, 0), want (0, 1, 0)
