# `ara why` vs no `ara why`: suite `diagnose` (2026-10-23)

Four broken outpost scenes, each described to the agent only by its symptom:

| Task | What is broken | Where the answer is |
|---|---|---|
| `why-bad-material` | the radio's `.material` outputs `pain` (the parameter is `paint`) | the material source; `ara check` does not compile materials |
| `why-missing-mesh` | the generator's mesh was re-exported as `generator_v2.glb` with a new ID | the reference resolves to nothing (ARA1302) |
| `why-culling-mask` | the workbench is on layer `editor`; the camera draws `default` and `props` | the layer field vs the camera's mask |
| `why-nan-barrel` | `Barrel.002` has zero mass; the play process reported physics dividing by zero, then a NaN position | the runtime report (persisted in `.ara/diagnostics.jsonl`) and the mass |

Every task adds the game camera first. Verifiers check the fix in the documents or
the material file, that `ara check` passes, and that scene changes went through
commands with a `why`.

**Configurations.** *With*: this branch's `ara` (the bus, `ara_why`, grouped
`ara_logs`, code docs). *Without*: `ara` built from `main` at `41cbc25`, passed
with `--ara`. The task setups and verifiers are the same arabench binary for both.
Agents: `claude:sonnet-5.5` and `codex:gpt-6-luna` (medium effort), one task at a
time per agent, six rounds per configuration (`why-with[-rN]-*`,
`why-without[-rN]-*`), on Windows 11 with other agents sharing the machine.

## Results (6 rounds × 4 tasks = 24 attempts per cell)

| | Claude with | Claude without | Codex with | Codex without |
|---|---|---|---|---|
| Passed | **24/24** | 20/24 | **24/24** | 23/24 |
| Total turns | **131** | 173 | **165** | 181 |
| Median turns | 6 | 7 | 6 | 6 |
| Input tokens (incl. cache reads) | 4.05 M | 4.16 M | 5.84 M | **4.10 M** |
| Output tokens | **22.4 k** | 27.8 k | **20.7 k** | 23.3 k |
| Agent time | **461 s** | 522 s | **496 s** | 505 s |
| Raw scene edits | 0 | 4 | 0 | 0 |
| Cost | $1.85 | $1.92 | (subscription) | |

Final engine only (rounds 5–6, compact `why`/`logs` answers):

| | Claude with | Claude without | Codex with | Codex without |
|---|---|---|---|---|
| Passed | 8/8 | 8/8 | 8/8 | 7/8 |
| Total turns | **44** | 66 | **53** | 56 |
| Input tokens | **1.41 M** | 1.65 M | 1.79 M | **0.99 M** |
| Cost | **$0.61** | $0.72 | | |

Per task, turns in rounds 1–6 (with / without; ✗ = failed):

| Task | Claude | Codex |
|---|---|---|
| `why-bad-material` | 6 6 6 5 5 6 / 7 7 9 9 7 9 | 15 11 15 10 13 9 / 12 12 14 7 5 2✗ |
| `why-missing-mesh` | 6 5 4 6 4 7 / 6 7 5✗ 8 7 7 | 5 5 8 3 4 7 / 4 4 9 5 9 4 |
| `why-culling-mask` | 3 3 3 3 3 3 / 6 7 6 6 8 6 | 6 2 2 3 4 3 / 7 4 4 5 6 6 |
| `why-nan-barrel` | 9 8 7 7 7 9 / 6✗ 6✗ 6✗ 6 11 11 | 9 5 7 6 9 4 / 4 18 12 4 12 12 |

## What it showed

- **Pass rate.** With `ara why` both agents passed every attempt. Without it Claude
  failed the NaN barrel 3 of 6 times: it found the playtest report by reading
  `.ara/diagnostics.jsonl` and fixed the mass by editing the scene JSON, which skips
  review (the verifier requires a command with a `why`). With `ara why` the answer
  came with a runnable `property.set` and Claude used it. Claude also failed one
  missing-mesh attempt and Codex one bad-material attempt (it stopped after
  explaining) without `ara why`.
- **Turns.** Claude needs 24% fewer turns with `ara why`; the culling mask goes from
  6–8 turns to 3 every time (ask, apply the fix, done). Codex needs 9% fewer.
- **Tokens.** Claude's input tokens are about even (−3% overall, −15% on the final
  engine) and its output is 19% lower. Codex uses 42% more input tokens with
  `ara why` available: it shifts from the shell (`ara check`, reading files) to MCP
  tools, whose JSON results are larger, and on the bad-material task it went
  looking for an engine command to edit a material (`ara_schema`, `ara_docs`) until
  the advice said plainly to edit the source file (round 6: 9 turns, 260 k tokens,
  down from 10–15 turns and 400–540 k).

## Changes the transcripts drove (all on this branch, rounds noted)

1. Round 2: a missing mesh source next to its sidecar became a check note instead
   of a low cause (the fixture has sidecars only, so every report mentioned it);
   free-text names resolve (`the workbench` → `Workbench`, `the barrels` →
   `Barrel`), preferring the entity the bus has errors about (`Barrel.002`).
2. Round 4: material errors point at the line that uses the bad identifier, quote
   it and suggest the declared name (`did you mean `paint`?`); the parser's own
   location was the end of the file.
3. Round 5: compact answers. Claude Code passes `structuredContent` (the result
   JSON), not `content`, to the model, so `why` now answers `{text, fixes}` and
   `ara_logs` its text unless asked for entries.
4. Round 6: material fixes say to edit the source file (no command edits it).

## Caveats

- The *without* runs still see the playtest report: the setup writes it with this
  branch's host, and the file is plain JSONL agents can read. The old engine had
  no such file, so the NaN delta is understated.
- One run per cell per round; turn counts move by a few between identical runs.
- `ara check` does not compile `.material` files, so in the bad-material task
  `ara check` passes before the fix; only `ara why` (or reading the file) finds it.

## Reproduce

```sh
# without: build main into its own tree, then
cargo run -p arabench -- run --agent claude:sonnet-5.5 --agent codex:gpt-6-luna --suite diagnose --label why-without --ara <main-tree>/target/debug/ara.exe
cargo run -p arabench -- run --agent claude:sonnet-5.5 --agent codex:gpt-6-luna --suite diagnose --label why-with
```
