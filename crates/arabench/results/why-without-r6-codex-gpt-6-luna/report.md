# arabench diagnose · codex:gpt-6-luna · why-without-r6

Started 2026-10-23T07:14:36Z on engine `e4325d6`. **3/4 passed (75%)**, 59 s of agent time.

| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |
|---|---|---|---|---|---|---|---|---|
| `why-bad-material` | diagnose | FAIL | 2 | 2 | 11s | 55k/1k | - | - |
| `why-missing-mesh` | diagnose | pass | 4 | 4 | 17s | 102k/1k | ARA1302 | - |
| `why-culling-mask` | diagnose | pass | 6 | 6 | 16s | 126k/1k | ARA1204 | - |
| `why-nan-barrel` | diagnose | pass | 12 | 12 | 15s | 116k/1k | - | - |

## Failures

### `why-bad-material`

- the radio material compiles: [Diagnostic { code: Code(3200), severity: Error, message: "unknown output value `pain`", location: Location { file: None, doc: None, record: None, path: None, line: Some(7), column: Some(1), command: None }, subjects: [], data: None, fixes: [] }]
