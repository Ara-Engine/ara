# arabench diagnose · claude:sonnet-5.5 · why-without

Started 2026-10-23T06:38:54Z on engine `b1456fd`. **3/4 passed (75%)**, 59 s of agent time, $0.30.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `why-bad-material` | diagnose | pass | 7 | 7 | 17s | 168k/1k | - | - |
| `why-missing-mesh` | diagnose | pass | 6 | 6 | 15s | 133k/2k | - | - |
| `why-culling-mask` | diagnose | pass | 6 | 6 | 13s | 172k/1k | - | - |
| `why-nan-barrel` | diagnose | FAIL | 6 | 6 | 14s | 117k/2k | - | 2 |

## Failures

### `why-nan-barrel`

- changed through commands with a `why`: without a why or from a file edit: tx_wxk6d9yz (external edit)
- raw edits: C:\Users\snowy\AppData\Local\Temp\arabench\claude-sonnet-5.5-why-nan-barrel-41504-213a25d4-3\proj\levels\outpost.scene
