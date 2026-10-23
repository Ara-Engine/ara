| Task | claude:sonnet-5.5 · baseline | claude:sonnet-5.5 · after | claude:sonnet-5.5 · after2 | codex:gpt-6-luna · baseline | codex:gpt-6-luna · after | codex:gpt-6-luna · after2 |
|---|---|---|---|---|---|---|
| `scene-floor-crate` | pass (8) | pass (6) | pass (5) | pass (11) | pass (12) | pass (5) |
| `scene-turn-and-raise` | pass (7) | pass (4) | pass (4) | pass (11) | pass (2) | pass (2) |
| `scene-group-barrels` | pass (8) | pass (8) | pass (5) | pass (17) | pass (4) | pass (9) |
| `component-beacon-light` | pass (8) | pass (4) | pass (4) | pass (14) | pass (9) | pass (4) |
| `property-halve-lights` | pass (10) | pass (5) | pass (8) | pass (13) | pass (4) | pass (3) |
| `component-remove-rigidbody` | pass (7) | pass (3) | pass (3) | **fail** (6) | pass (3) | pass (3) |
| `prefab-stack-crates` | pass (10) | pass (8) | pass (7) | pass (12) | pass (6) | pass (6) |
| `prefab-tint-one-crate` | pass (9) | pass (4) | pass (4) | pass (12) | pass (10) | pass (7) |
| `prefab-revert-keep-position` | pass (9) | pass (7) | pass (8) | pass (12) | pass (8) | pass (15) |
| `prefab-blue-bulb` | pass (11) | pass (5) | pass (6) | pass (9) | pass (4) | pass (6) |
| `hierarchy-radio-to-bench` | pass (9) | pass (4) | pass (8) | **fail** (9) | pass (2) | pass (11) |
| `hierarchy-order-waypoints` | pass (10) | pass (7) | pass (6) | pass (8) | pass (4) | pass (3) |
| `refs-fix-broken` | pass (3) | pass (7) | pass (5) | pass (10) | pass (6) | pass (12) |
| `merge-resolve-conflict` | pass (6) | pass (5) | pass (5) | pass (13) | pass (9) | pass (6) |
| `diag-fix-typos` | pass (4) | pass (4) | pass (2) | pass (4) | pass (15) | pass (6) |
| `schema-door-fields` | pass (10) | pass (5) | pass (6) | pass (14) | pass (6) | pass (9) |
| `undo-other-agent` | pass (8) | pass (4) | pass (4) | pass (3) | pass (3) | pass (3) |
| `docs-cheatsheet` | pass (7) | pass (6) | pass (6) | pass (4) | pass (5) | pass (4) |
| `batch-watchtower` | pass (8) | pass (6) | pass (6) | pass (7) | pass (8) | pass (8) |
| `dryrun-clear-ruins` | pass (9) | pass (5) | pass (5) | pass (7) | pass (7) | pass (6) |
| `multi-storage-room` | pass (11) | pass (8) | pass (10) | pass (14) | pass (17) | pass (21) |
| `multi-qa-fixes` | pass (11) | pass (8) | pass (7) | pass (24) | pass (6) | pass (6) |
| **Passed** | **22/22 (100%)** | **22/22 (100%)** | **22/22 (100%)** | **20/22 (91%)** | **22/22 (100%)** | **22/22 (100%)** |
| Median turns | 9 | 5 | 6 | 11 | 6 | 6 |
| Total turns | 183 | 123 | 124 | 234 | 150 | 155 |
| Agent time | 933 s | 465 s | 604 s | 758 s | 508 s | 700 s |
| Tokens in / out | 5070k / 32k | 3816k / 23k | 3848k / 23k | 5532k / 30k | 4134k / 20k | 4073k / 21k |
| Cost | $2.13 | $1.80 | $1.81 | - | - | - |
| Failed tool calls with ARA codes | 7 | 10 | 4 | 39 | 16 | 21 |
| Raw document edits | 6 | 8 | 5 | 7 | 3 | 3 |
