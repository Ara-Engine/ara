# ara-hub-service

User-level Ara Hub backend (D25). No UI and no gameplay state. The native Hub
uses the same authenticated JSON-RPC API as global MCP. One service per config
directory, enforced by an OS file lock; projects run in separate processes.

## Public API

- `Hub::open(HubConfig)`; `Hub::call_for(conn, method, params)` / `Endpoint`.
- `serve(config, port)` runs authenticated localhost WebSocket RPC.
- `connect_or_start(directory, ara_executable)` auto-starts `ara hub serve`.
- `config_dir()`: `ARA_CONFIG_DIR`, else `%APPDATA%/Ara` on Windows,
  `~/Library/Application Support/Ara` on macOS, `$XDG_CONFIG_HOME/ara` or
  `~/.config/ara` elsewhere.
- `TemplateSource { list, instantiate }`; default `DirectoryTemplates` lists
  built-in `blank` plus child directories containing `ara.project.json`. Names and
  `description` come from that marker; the preview is `thumbnail.png`, else
  `evidence/<main-scene>-player.png`. An `assets.lock.json` beside the templates
  (the samples lock) sets `setup {message,command}` while pinned files under
  `assets/third_party/` are missing; `hub.create` then fails with ARA1913.
- When the engine runs from `<checkout>/target/<profile>/`, `Hub::open` layers the
  checkout's `samples/` after the configured templates (installed builds ship them
  in `<install>/templates`).
- `Registry`, `ProjectEntry`, `EngineBuild` are durable data, never live handles.

## Wire contract for the Hub UI

Connect using `ara_rpc::Client::for_project(config_dir.join("hub"))` (the reused
transport discovery is `hub/.ara/daemon.json`; do not render its token). All
params/results use camelCase. No engine session is needed for `hub.*`.

| Method | Params | Result / what to render |
| --- | --- | --- |
| `hub.projects` | `{}` | `{projects,revision}`: cards in registry order, each `{id,name,path,lastOpened,thumbnail,engineVersion}`; lastOpened is Unix ms or null |
| `hub.register` | `{path,name?,engineVersion?}` | Project card; register an existing absolute directory without copying it |
| `hub.update` | `{project,name?,thumbnail?,engineVersion?} (thumbnail:null clears)` | Updated card; rename, preview image, version picker; close before changing engine pin |
| `hub.remove` | `{project}` | `{removed:id}`; unregister only, preserving files |
| `hub.templates` | `{}` | `{templates:[{id,name,thumbnail,description?,setup?}]}`; template tiles in Create |
| `hub.create` | `{template,name,path,engineVersion?,dryRun?}` | New registered project card; path must be absolute and not exist. Every problem is one diagnostic whose `location.path` is the field (`name`, `path`, `template`, `engineVersion`); `dryRun` returns `{valid,path}` without creating |
| `hub.open` | `{project,headless?}` | Status; false/default opens native editor, true starts headless |
| `hub.close` | `{project}` | Closed status after graceful flush/shutdown |
| `hub.status` | `{project?}` | One status or `{projects:[status],revision}` |
| `hub.engines` | `{}` | `{engines:[{version,executable,editor}]}`; installed builds for each project's pin picker |
| `hub.engine.register` | `{engine:{version,executable,editor}}` | Persist an installed version; executable paths must exist and be absolute |
| `hub.engine.remove` | `{engine:{version,executable,editor}}` | Remove only that exact build; reject while pinned projects are open |
| `hub.activity` | `{project?}` | `{activity:[{project:id,participants,presence}]}`; attached human/editor and agent rows, current selection/camera/task |
| `hub.subscribe` | `{}` | `{revision}`; subsequent `hub.changed {revision,kind,project}` notifications |
| `hub.unsubscribe` | `{}` | `{ok:true}`; also automatic on socket close |

`project` accepts a registered stable ID, unique name or canonical path. Agents
cannot target an unregistered directory through global MCP. A status is
`{project:card,state:"open"|"closed",pid,port,headless}`; closed fields are null.
Use state, not stale discovery files, for the open indicator. A change notification
invalidates the appropriate view: refresh projects/status/activity; revision is a
monotonic counter within this Hub run, not an on-disk revision. Subscribe before
initial reads to avoid missing a change. No replay history is promised; refresh
all views after reconnect. Do not render lifecycle calls as approval or review.

Render: project cards (name, thumbnail fallback, recent time, path, pinned engine,
open/headless/editor badge), Create (template/name/destination/version), Open and
Open Headless, Close, Rename, Remove from list, installed engines, agent/editor
activity. Errors are standard RpcError diagnostics: show message and runnable
fixes. The native UI is `crates/ara-hub`.

`hub.resolve {project}` returns a card. `hub.catalog {project?}` returns the running project catalog or the core method
catalog without opening any project. `hub.call {project,method,params}` opens a
registered project headless on demand and forwards to its authority. Each Hub
socket/project pair has a distinct downstream socket and session attribution.
MCP responses include `notices` from participant-specific conflict events;
normal replica op streams are filtered so idle agents do not accumulate them.
`hub.shutdown {}` stops only the user service; open project processes survive and
are rediscovered after restart. Hub exit never implicitly closes user projects.

## Persistence and lifecycle

`registry.json` is canonical JSON, replaced atomically. A project's canonical
`ara.project.json` owns `engineVersion`. Alpha registers `0.0.1-alpha`; the registry
supports multiple `EngineBuild`s with separate CLI/editor paths. Opening an
editor flushes/stops an existing headless process and launches `ara-editor <root>`:
the existing editor owns the project authority/lock. Switching mode drops old RPC
routes; clients reconnect. There is no second editor authority for one project.
Process startup errors point to `.ara/hub-process.log`; Hub startup to
`hub-start.log`. Daemons start without a console window on Windows; Hub auto-start uses
CreateProcessW without handle inheritance to preserve captured CLI/MCP EOF.
A project's root `thumbnail.png` becomes its card preview on register; `hub.create`
copies the template's preview there. Templates must not contain symlinks; `.ara`, `.git`, `target` and `evidence` are skipped.
A failed custom template can leave its newly created destination for inspection.

## One-time global MCP installation

`ara hub install-mcp --client claude` prints the `mcpServers.ara` JSON entry for
Claude Code's user configuration (`~/.claude.json`). Equivalent registration:
`claude mcp add --scope user ara -- ara mcp`.
`ara hub install-mcp --client codex` prints `[mcp_servers.ara]` TOML for
`~/.codex/config.toml`. Both run `ara mcp` once per client, with no root.
Register existing projects first with `ara hub add <absolute-path>`; create with
`ara hub create <absolute-path> --name Name --template blank`.
Use `ara_projects {action:"list"}` then `ara_session {action:"open",project:"Name"}`
or give `project` on any existing tool. Sessions never cross projects.
Per-project compatibility: `ara mcp --root <project>` and optional `.mcp.json`
(`args:["mcp","--root","."]`) retain the standalone project daemon behavior.

## Tests and gates

`CARGO_BUILD_JOBS=2 cargo test -p ara-hub-service` covers durable registry/templates.
`cargo test -p ara --test hub` runs real user Hub + global stdio MCP + two project
daemons, concurrent clients, registry metadata, subscriptions, and install output.
`cargo test -p ara --test engine` retains per-project MCP/HTTP coverage.
`cargo clippy -p ara-hub-service --all-targets -- -D warnings`; scoped rustfmt.
No new root workspace dependency: member discovery is already `crates/*`.
