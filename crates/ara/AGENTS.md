# ara (CLI and daemon)

The `ara` binary: the single entry point for humans and agents. File commands work
offline on project files; protocol commands go through the project's daemon when
it runs (`.ara/daemon.json`), else an in-process `ara_host::Host`.

## Commands

| Command | Does | Exit 1 when |
|---|---|---|
| `ara init [dir] [--name n]` | `ara.project.json` (`name`, `main` scene), `AGENTS.md` (conventions), `.gitattributes` (merge driver), `.gitignore` (`.ara/`), `.mcp.json`, `levels/main.scene`; configures `merge.ara` in a git work tree | the project exists (ARA1902) |
| `ara fmt [--check] <files>` | canonical rewrite, repairing `@path#id` references | `--check` and a file would change; a file has errors |
| `ara check [--fix] [files]` | schema, reference, structure and prefab checks (all documents by default); `--fix` first applies every diagnostic with exactly one `rename`/`set`/`remove` fix | any error |
| `ara schema [names..] [--out dir]` | Schema DB summary; names: each one's doc page (fields, units, defaults); `--json` = `reflection.json`; `--out` writes JSON Schemas | project types are invalid; unknown name |
| `ara merge [-o out] <base> <ours> <theirs>` | semantic 3-way merge over `ours` (git `%O %A %B`) | conflicts |
| `ara resolve <file> [--ours\|--theirs] [--pick r:path=side]..` | settles `$conflicts` by side (`ours`, `theirs`, `base`) | conflicts left |
| `ara docs --llms [--out dir]` | `llms.txt`, `llms-full.txt`, per-type/command/method pages (default `.ara/docs`) | never |
| `ara serve [--port n] [--stdio]` | the daemon: EP over WebSocket (+stdio), MCP at `POST /mcp`; `--json` prints `{pid, port, url, mcp}` once listening | it cannot start |
| `ara mcp` | MCP on stdio; a client of the daemon, or the daemon itself if none runs | it cannot start |
| `ara do [cmd] [--doc d] [--args json\|@file\|-] [-p k=v].. [--as $s] [--why w] [--dry-run]` | one-call `cmd.apply`; without `cmd`, `--args` is a batch `{commands: [{cmd, args, as}], why, doc}` | the batch fails |
| `ara query [q] [--doc] [--fields a,b] [--limit] [--cursor]` | `query` | bad query |
| `ara inspect [--doc d] [entities..] [--depth n]` | `inspect` (scene-to-text; IDs or names) | unknown doc/entity |
| `ara history [--doc] [--by author] [--limit]`, `ara review` | `history.list`, `review.list` | |
| `ara undo [txn]`, `ara redo` | `history.undo/redo` for `--author` | nothing to undo, conflicts |
| `ara accept [txn] [--entity e] [--all]`, `ara revert [txn] [--entity e]` | `review.accept/revert` | nothing to review |
| `ara logs [--since] [--code].. [--severity] [--entity] [--limit]` | `logs` | |

Global: `--root <dir>`, `--json` (every command), `--author human|agent:<name>`
(protocol commands; default `ARA_AUTHOR`, else the agent whose shell runs the CLI:
`CODEX_THREAD_ID` → `agent:codex`, `CLAUDECODE` → `agent:claude-code`, else
`human`). Exit 2: the command could not run (no
project, IO, lock held for over 10 s); with `--json` the diagnostics are printed as
`{"ok": false, "diagnostics": [...]}`.

## Project root

`--root`, else the nearest ancestor of the current directory with
`ara.project.json`, else (file commands) the nearest one above the first file.
Never the bare current directory: without a marker every project command fails
with ARA1901 and a `Cli ["init", dir]` fix. Scans skip hidden dirs (`.git`,
`.ara`, `.refs`), `target/`, `node_modules/`, `spikes/` and nested projects.

## Code map

- `main.rs`: clap definitions, protocol-command → method mapping, default author,
  exit codes.
- `relaxed.rs`: JSON with optional quotes for `--args`/`-p` (PowerShell 5.1 strips
  the double quotes of native arguments).
- `project.rs`: root resolution (wraps `ara_host::project`).
- `engine.rs`: daemon-or-offline endpoint, per-command session, output.
- `commands/`: `init`, `serve` (daemon + MCP glue: `Daemon` implements
  `ara_rpc::Handler`; stdio MCP is per-connection, `/mcp` stateless), `protocol`
  (args and summaries), `docs`, `fmt`, `check` (+ `--fix`), `schema`, `merge`,
  `resolve`, `edit` (record/field-path edits of a document file's JSON).

## Gotchas

- Protocol commands each begin and end a session; offline they replay the
  journal, so history, review and undo persist between runs.
- `ara mcp` without a daemon *becomes* the daemon (writes `.ara/daemon.json`), so
  stdout carries only MCP; logs go to stderr.
- `merge` reads without the asset index (git passes temp files), so it never
  rewrites references; run `ara fmt` afterwards. `init` sets the driver with
  `--root <project dir relative to the work tree>`.
- `check` loads every project document, even for a single file.
- JSON output goes through `ara_json::to_canonical_string`, so it is stable.
- Agent attribution is environment-driven: tests clear `CLAUDECODE`,
  `CODEX_THREAD_ID` and `ARA_AUTHOR` (`ara_env`) so they pass from any shell.
- `check --fix` applies only diagnostics with exactly one fix; ambiguous
  suggestions (tied reference matches) are offered, never applied. Plain
  `check` prints a hint (and JSON `fixable`) when some could be.

## Testing

`cargo test -p ara`. `tests/cli.rs`: file commands on `tests/fixtures/`
(`project/`, `broken/`, `merge/`, `messy.scene`; the `reflection` snapshot is the
whole Schema DB), `resolve` and `check --fix`. Unit tests: `relaxed`, `edit`. `tests/engine.rs`: root discovery and skips, `init`, offline
protocol commands across runs, `docs --llms`, `ara mcp` becoming the daemon (MCP
over Streamable HTTP too), and the end-to-end test: `ara serve` → `ara mcp`
client → session, batch with `why`, query, revert, file checked on disk (the CLI
sees the same daemon); agent shells (attribution, required `why`, quote-stripped
`--args`, no `--doc`).
