# arabench diagnose · claude:sonnet-5.5 · why-without-r2

Started 2026-10-23T06:44:48Z on engine `baa7b61+dirty`. **3/4 passed (75%)**, 75 s of agent time, $0.29.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `why-bad-material` | diagnose | pass | 7 | 7 | 21s | 146k/2k | - | - |
| `why-missing-mesh` | diagnose | pass | 7 | 7 | 18s | 168k/2k | - | - |
| `why-culling-mask` | diagnose | pass | 7 | 7 | 19s | 170k/2k | - | - |
| `why-nan-barrel` | diagnose | FAIL | 6 | 6 | 17s | 123k/2k | - | 2 |

## Failures

### `why-nan-barrel`

- changed through commands with a `why`: without a why or from a file edit: tx_gsjt93jp (external edit)
- raw edits: C:\Users\snowy\AppData\Local\Temp\arabench\claude-sonnet-5.5-why-nan-barrel-7668-1ddc3468-3\proj\levels\outpost.scene
