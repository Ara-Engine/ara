# arabench collab · codex:gpt-6-luna · collab

Started 2026-10-23T10:43:14Z on engine `62b8ce4`. **1/2 passed (50%)**, 80 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `collab-avoid-claimed` | collab | FAIL | 14 | 14 | 44s | 365k/3k | ARA1101 ARA2500 | - |
| `collab-branch-claimed` | collab | pass | 12 | 12 | 36s | 509k/2k | ARA1101 ARA1106 ARA1804 | - |

## Failures

### `collab-avoid-claimed`

- `e_wlight1` doubled: got Some(1200.0), want 2400
- `e_ylamp1/e_bulb01` doubled: got Some(800.0), want 1600
- changed through commands with a `why`: no transaction touched them
