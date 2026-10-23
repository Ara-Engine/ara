# arabench v0 results (2026-10-23)

> **Suite `diagnose` (`ara why`):** four broken scenes, 6 rounds per configuration.
> With `ara why` both agents passed 24/24 (without: Claude 20/24, Codex 23/24),
> Claude in 24% fewer turns and Codex in 9% fewer; Codex used more input tokens.
> Details: [`diagnose.md`](diagnose.md) (`why-with[-rN]-*`, `why-without[-rN]-*`).

> **Suite `collab` (D21 claims and branch sessions):** two tasks with a teammate
> holding claims. Run 1: Claude 2/2, Codex 1/2 (queued unclaimed work in a branch
> too); after rewording ARA2500, run 2: both 2/2. Details: [`collab.md`](collab.md).

Suite v0 (22 tasks) run by Claude Code with `claude:sonnet-5.5` and Codex with
`codex:gpt-6-luna` (medium effort), once per engine version, 3 tasks at a time,
on Windows 11. Each attempt is a fresh `ara init` project plus the outpost fixture,
driven over the generated MCP config with `ara` on `PATH`.

| Label | Engine | What changed |
|---|---|---|
| `baseline` | `ce6c8b3` (harness `642e6ad`) | the merged M1 kernel, untouched |
| `after` | `d3fa6b3` | round 1: selectors, default `doc`, apply summaries, required agent `why`, optional MCP session, command list in `ara_apply`, relaxed CLI JSON, agent attribution, `ara resolve`, `ara check --fix`, reference suggestions |
| `after2` | `09cc127` | round 2: `as` without `$`, assets without `@`, instance-root addressing, `ara do` batches, `--fix` hint, `ara schema <names>`, `describe {name}` |

Full table: [`compare.md`](compare.md). Per run: `<label>-<agent>/report.md` and
`results.json`.

| | Claude baseline | after | after2 | Codex baseline | after | after2 |
|---|---|---|---|---|---|---|
| Passed | 22/22 | 22/22 | 22/22 | 20/22 | 22/22 | 22/22 |
| Median turns | 9 | 5 | 6 | 11 | 6 | 6 |
| Total turns | 183 | 123 | 124 | 234 | 150 | 155 |
| Agent time | 933 s | 465 s | 604 s | 758 s | 508 s | 700 s |
| Input tokens | 5.07 M | 3.82 M | 3.85 M | 5.53 M | 4.13 M | 4.07 M |
| Failed calls with ARA codes | 7 | 10 | 4 | 39 | 16 | 21 |
| Raw document edits | 6 | 8 | 5 | 7 | 3 | 3 |
| Cost | $2.13 | $1.80 | $1.81 | (subscription) | | |

## What the baseline showed

- **Codex drove the CLI, not MCP** (173 shell calls vs 18 `ara_inspect`), and Windows
  PowerShell 5.1 strips the double quotes of native arguments: `--args
  '{"entity":…}'` arrived as `{entity:…}`. ARA1602 ("`--args` is not JSON") in 11
  of 22 tasks (17 failed calls); `-p entities='["e_radio1"]'` became a string
  (ARA1102, the radio failure).
- **CLI edits by agents were attributed to `human`** (default `--author`), so they
  skipped review; one Codex change had no `why` (the rigidbody failure).
- **Claude paid fixed overhead per task**: `session.begin`, a documents query and
  `schema describe` before each command (40 `ara_schema` calls); ARA1803 from
  calling tools in parallel before `begin` returned.
- `ara query Radio` failed (ARA1808: "unknown component"); `doc` was required even
  when the entities named it (ARA1506 once per agent); `as` + `args.id` with the same
  symbol was rejected; merge conflicts and typos were hand-edited JSON (ARA1001
  syntax errors, three in one Codex run).

## Fixes, in order of effect

1. **Relaxed CLI JSON + `--args @file|-`**: Codex ARA1602 went from 17 to 0 (1 in
   after2).
2. **Agent attribution in the CLI** (`CODEX_THREAD_ID`, `CLAUDECODE`, `ARA_AUTHOR`)
   **and a required `why` for agent changes** (ARA1610, reported with every dry-run
   error so one retry fixes all).
3. **Optional MCP session over `ara mcp`** (per-connection adapter); `session.begin`
   also lists documents. Claude's `ara_session` calls went from 21 to 2–3.
4. **`ara_apply` lists every command with its args** (generated from the registry):
   Claude's `ara_schema` calls went from 40 to 8.
5. **Default `doc`** (the document holding the named entities, else the main scene
   recorded by `ara init`; `outpost` is enough) and **name selectors**
   (`name:Crate`, `name:Floor/Crate`; a bare name is ARA1102 with a `set` fix per
   matching ID; ARA1609 for ambiguity; ARA1204 points at the document that has the
   ID).
6. **`cmd.apply` text**: where it applied and every created/modified/deleted entity
   with ID, name and symbol (dry runs say "would").
7. **File-level repairs**: `ara resolve` (merge conflicts by side; ARA1209 carries
   the commands), `ara check --fix` (applies single fixes; plain `check` hints at
   it), closest-asset suggestions on ARA1302 (ties become several fixes, applied
   by none).
8. Round 2: `as` without `$`, assets without `@`, `instance/<root ID>`, `ara do`
   batches, `ara schema <names>`.

Every fix has a focused test in its crate (`ara-cmd`, `ara-host`, `ara-mcp`,
`ara-doc`, `ara-diag`, `ara`).

## Caveats

- One run per cell: turn counts move by a few between identical runs (both
  baseline Codex failures passed when re-run on the baseline binary:
  [`baseline-repro-codex-gpt-6-luna`](baseline-repro-codex-gpt-6-luna)). Pass rate
  is saturated for Claude, so turns, failed calls and raw edits carry the signal.
- Verifier fixes during the work, with the affected task re-run on the same engine:
  `component-beacon-light` (colors are linear: Codex's correct `(1, 0.214, 0)` was
  rejected; baseline re-run) and `dryrun-clear-ruins` (`ara schema entity.delete`
  counted as a delete; after2 re-run).
- The trimmed transcripts of the two baseline Codex failures were deleted by the
  `--merge` re-run (fixed: a merge now replaces only its tasks' transcripts). The
  one kept failure transcript is from a probe of the round-1 engine:
  [`probe-after-codex-gpt-6-luna/transcripts/scene-floor-crate.jsonl`](probe-after-codex-gpt-6-luna/transcripts/scene-floor-crate.jsonl)
  (Codex piped a batch into `ara do` without a command; round 2 added batches).
- `after2` ran while `cargo test --workspace` compiled, so its agent times are
  inflated; compare turns and tokens instead.
- The users' own memory files (`CLAUDE.md`, `~/.codex/AGENTS.md`) load in both CLIs.

## Remaining friction

- Claude Code defers MCP tools: a `ToolSearch` call starts almost every task.
- Codex prefers the CLI and reads whole `--json` dumps; `ara schema <names>` now
  answers lookups, but field paths (`Transform.position`, fixed after the last
  run, unmeasured) and document kinds (`scene`) missed it in after2 (ARA1106 ×9).
- A broken prefab reference can only be fixed in the file: no command relinks an
  instance, and tied suggestions (`lantern` → `crate` or `lamp`) need a choice.
- Merge resolution and `check --fix` are CLI-only: `$conflicts` are document
  metadata outside the op model, so MCP-only agents cannot settle them.
- Setting a field does not drop a misspelled sibling (`Collider.sise`), so the
  file keeps failing `ara check` until `--fix` or a hand edit.
- The storage-room task still takes 10–21 turns; agents verify with whole-scene
  inspects. Capture, play and test (ARA1806 today) are the next suite areas.

## Reproduce

```sh
cargo run -p arabench -- run --agent claude:sonnet-5.5 --agent codex:gpt-6-luna --label mine --jobs 3
cargo run -p arabench -- compare results/baseline-claude-sonnet-5.5/results.json results/mine-claude-sonnet-5.5/results.json
```
