# arabench v0 · codex:gpt-6-luna · probe2

Started 2026-10-23T01:40:50Z on engine `d3fa6b3+dirty`. **4/5 passed (80%)**, 158 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `scene-floor-crate` | assembly | FAIL | 5 | 5 | 23s | 126k/1k | - | - |
| `diag-fix-typos` | diagnostics | pass | 15 | 15 | 55s | 611k/3k | ARA1001×2 ARA1101×2 | 4 |
| `schema-door-fields` | schema | pass | 5 | 5 | 19s | 138k/1k | - | - |
| `batch-watchtower` | batch | pass | 10 | 10 | 30s | 335k/2k | ARA1101 ARA1105 | - |
| `multi-storage-room` | multi-step | pass | 9 | 9 | 31s | 269k/2k | ARA1204 | - |

## Failures

### `scene-floor-crate`

- one `Floor` in levels/main.scene: found 0
