# ara (CLI and daemon)

The `ara` binary: the single entry point for humans and agents. File commands work
offline on project files; protocol commands go through the project's daemon when
it runs (`.ara/daemon.json`), else an in-process `ara_host::Host`.

## Commands

| Command | Does | Exit 1 when |
|---|---|---|
| `ara doctor` | Project-free .NET 10 SDK/runtime and bundled shader tool checks, with download fixes | a prerequisite is missing |
| `ara new [dir] [--name n] [--configure-git]` | `ara.project.json` (`name`, `main` scene), `AGENTS.md` (conventions), `.gitattributes` (merge driver), `.gitignore` (`.ara/`, `/builds/`), `.mcp.json`, `content/scenes/main.scene`; see driver setup below | the project exists (ARA1902) |
| `ara project migrate [--dry-run] [--support-dir dir]` | preview or apply legacy content/settings migration, preserving IDs, references and journal history | collisions or an open project |
| `ara fmt [--check] <files>` | canonical rewrite, repairing `@path#id` references | `--check` and a file would change; a file has errors |
| `ara check [--fix] [files]` | schema, reference, structure and prefab checks (all documents by default); `--fix` first applies every diagnostic with exactly one `rename`/`set`/`remove` fix | any error |
| `ara schema [names..] [--out dir]` | Schema DB summary; names: each one's doc page (fields, units, defaults); `--json` = `reflection.json`; `--out` writes JSON Schemas | project types are invalid; unknown name |
| `ara merge [-o out] <base> <ours> <theirs>` | semantic 3-way merge over `ours` (git `%O %A %B`) | conflicts |
| `ara resolve <file> [--ours\|--theirs] [--pick r:path=side]..` | settles `$conflicts` by side (`ours`, `theirs`, `base`) | conflicts left |
| `ara docs --llms [--out dir]` | `docs.json`, `llms.txt`, `llms-full.txt`, per-type/command/method pages (default `.ara/docs`); `--json` prints the summary | never |
| `ara serve [--port n] [--stdio]` | the daemon: EP over WebSocket (+stdio), MCP at `POST /mcp`; `--json` prints `{pid, port, url, mcp}` once listening | it cannot start |
| `ara mcp` | Global MCP through the user Hub; `--root` retains per-project mode | it cannot start |
| `ara do [cmd] [--doc d] [--args json\|@file\|-] [-p k=v].. [--as $s] [--why w] [--dry-run]` | one-call `cmd.apply`; without `cmd`, `--args` is a batch `{commands: [{cmd, args, as}], why, doc}` | the batch fails |
| `ara query [q] [--doc] [--fields a,b] [--limit] [--cursor]` | `query` | bad query |
| `ara inspect [--doc d] [entities..] [--depth n]` | `inspect` (scene-to-text; IDs or names) | unknown doc/entity |
| `ara history [--doc] [--by author] [--limit]`, `ara review` | `history.list`, `review.list` | |
| `ara undo [txn]`, `ara redo` | `history.undo/redo` for `--author` | nothing to undo, conflicts |
| `ara run [--scene s] [--ticks n\|--duration seconds]` | headless fixed simulation with hash, logs and diagnostics | invalid request or runtime fault |
| `ara capture [--scene s] [--camera id] [--path png] [--width n] [--height n] [--preset low\|medium\|high\|epic]` | offscreen scene PNG | render failure |
| `ara accept [txn] [--entity e] [--all]`, `ara revert [txn] [--entity e]` | `review.accept/revert` | nothing to review |
| `ara logs [--since] [--code].. [--severity] [--entity] [--subject].. [--origin] [--grep] [--all] [--budget] [--limit]` | `logs`: grouped by code (`--all`: one by one) | |
| `ara why <question..> [--doc d] [--list]` | `why`: ranked causes, evidence, runnable fixes (`name:Crate not visible`, `ARA1302`, `@meshes/x.glb`, `is the frame slow`) | |

Global: `--root <dir>`, `--json` (every command), `--author human|agent:<name>`
(protocol commands; default `ARA_AUTHOR`, else the agent whose shell runs the CLI:
`CODEX_THREAD_ID` → `agent:codex`, `CLAUDECODE` → `agent:claude-code`, else
`human`). Exit 2: the command could not run (no
project, IO, lock held for over 10 s); with `--json` the diagnostics are printed as
`{"ok": false, "diagnostics": [...]}`.

## Project root
`ara profile compare baseline.json current.json` runs without a project or daemon.
It prints a p95 delta table (`--json` for structured output); exit 1 means a
regression, missing metrics or incomplete capture. Thresholds are `--relative`
(default 0.05) and `--absolute-ns` (default 1000).


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
  `resolve`, `edit` (record/field-path edits of a document file's JSON), `why`
  (`ara why` and the `ara logs` args).

## Gotchas

- Protocol commands each begin and end a session; offline they replay the
  journal, so history, review and undo persist between runs.
- `ara mcp --root` without a daemon becomes the project daemon. Global `ara mcp`
  auto-starts the user Hub and only routes registered projects; stdout is MCP only.
- `merge` reads without the asset index (git passes temp files), so it never
  rewrites references; run `ara fmt` afterwards. `init` configures the root-agnostic
  `ara merge %O %A %B` driver only at the git work-tree root or with
  `--configure-git`. Merge discovers the project from its working directory or inputs.
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

`tests/init_git.rs`: Git driver scope and merge root discovery in external temp dirs.

`cargo test -p ara --test hub_ui`: Hub view-model lifecycle using Cargo's CLI binary;
the dev-dependency makes Hub changes select this integration test in ara-dev.

`tests/hub.rs` retains MCP requests, raw responses and call timing in
`target/hub-tests/<run>/mcp-{A,B,projects}.jsonl`. Failures name the agent,
method and call ID; preserve the complete JSON when reporting a startup flake.

`ara hub serve/add/remove/rename/projects/templates/create/open/close/status/engines/
activity/install-mcp` manage the user service. Install output is one-time global
`ara hub register-engine/unregister-engine` register this exact installed build;
`ara hub shutdown` stops the service without closing user projects.
`register-engine/unregister-engine --no-start` reuse RPC or lock the registry offline;
they never spawn a service. `install-mcp` prints the current absolute executable path.
Claude JSON or Codex TOML. Wire API and UI contract: ../ara-hub-service/AGENTS.md.
