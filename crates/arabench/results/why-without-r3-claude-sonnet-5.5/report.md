# arabench diagnose · claude:sonnet-5.5 · why-without-r3

Started 2026-10-23T06:54:26Z on engine `9705198+dirty`. **2/4 passed (50%)**, 74 s of agent time, $0.30.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `why-bad-material` | diagnose | pass | 9 | 9 | 22s | 169k/2k | - | - |
| `why-missing-mesh` | diagnose | FAIL | 5 | 5 | 17s | 122k/2k | - | 2 |
| `why-culling-mask` | diagnose | pass | 6 | 6 | 14s | 172k/1k | - | - |
| `why-nan-barrel` | diagnose | FAIL | 6 | 6 | 22s | 123k/2k | - | 2 |

## Failures

### `why-missing-mesh`

- changed through commands with a `why`: without a why or from a file edit: tx_4wqscfgs (external edit)
- raw edits: C:\Users\snowy\AppData\Local\Temp\arabench\claude-sonnet-5.5-why-missing-mesh-46380-342303f0-1\proj\levels\outpost.scene

### `why-nan-barrel`

- changed through commands with a `why`: without a why or from a file edit: tx_0qr3sbj0 (external edit)
- raw edits: C:\Users\snowy\AppData\Local\Temp\arabench\claude-sonnet-5.5-why-nan-barrel-46380-1f7190ac-3\proj\levels\outpost.scene
