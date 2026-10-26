# ara-hub-service

User-level Ara Hub backend (D25), shared by the native Hub and global MCP.
One service per config directory, enforced by an OS lock; projects run separately.

## Public API

- `Hub::open(HubConfig)`; `Hub::call_for(conn, method, params)` / `Endpoint`.
- `serve(config, port)` runs authenticated localhost WebSocket RPC.
- `connect_or_start(directory, ara_executable)` auto-starts `ara hub serve`;
  `connect_checked` also reports `update_pending` for a busy older service.
- `config_dir()`: `ARA_CONFIG_DIR`, else `%APPDATA%/Ara` on Windows,
  `~/Library/Application Support/Ara` on macOS, `$XDG_CONFIG_HOME/ara` or
  `~/.config/ara` elsewhere.
- Checkout/test binaries reject the real profile before discovery or writes;
  installed services also reject worktree engine/editor registrations there.
  Explicit overrides use `HubConfig::for_directory` and the same validation.
- `TemplateSource { list, instantiate }`; default `DirectoryTemplates` lists
  built-in `blank` plus child directories containing `ara.project.json`. Names and
  `description` come from that marker; the preview is `thumbnail.png`, else
  `evidence/<main-scene>-player.png`. An `assets.lock.json` beside the templates
  (the samples lock) sets `setup {message,command}` for missing third-party pins
  or generated kit sources/local glTF companions. `hub.create` fails with ARA1913
  before writing; `cargo run -p ara-samples -- kits <sample>` restores kits.
- Run from `<checkout>/target/<profile>/`, `Hub::open` layers the checkout's `samples/`
  after the configured templates (installed builds ship `<install>/templates`).
- Templates resolve from registered engines in descending semantic version order,
  then configured/service templates and checkout samples. Each tile's `engineVersion`
  identifies its source; create defaults to that pin and uses that source's asset lock.
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
| `hub.templates` | `{}` | `{templates:[{id,name,thumbnail,description?,setup?,engineVersion?}]}`; source engine pins for Create |
| `hub.create` | `{template,name,path,engineVersion?,dryRun?}` | New registered project card; path must be absolute and not exist; missing parents are created. Every problem is one diagnostic whose `location.path` is the field (`name`, `path`, `template`, `engineVersion`); `dryRun` returns `{valid,path}` without creating |
| `hub.open` | `{project,headless?}` | Status; false/default opens native editor, true starts headless |
| `hub.close` | `{project}` | Closed status after graceful flush/shutdown |
| `hub.status` | `{project?}` | One status or `{projects:[status],revision,build}`; build is the service executable's SHA-256 |
| `hub.engines` | `{}` | `{engines:[{version,executable,editor,managed}]}`; installed builds for each project's pin picker; managed builds support Uninstall |
| `hub.engine.register` | `{engine:{version,executable,editor}}` | Persist an installed version; executable paths must exist and be absolute |
| `hub.engine.remove` | `{engine:{version,executable,editor}}` | Remove only that exact build; reject while pinned projects are open |
| `hub.engines.available` | `{channel?:"stable"|"preview"}` | `{releases:[signed manifest],status,checking,repo,channel}`; first call starts a background check; preview includes stable |
| `hub.engine.install` | `{version,hubVersion?}` | Job `{id,kind,version,state,progress:{received,total},result,error}` immediately; verification/extraction run off the authority thread |
| `hub.engine.uninstall` | `{version}` | `{removed:version}`; Hub-managed files only, rejects open pins |
| `hub.downloads` | `{}` | `{jobs:[job]}`; state is `running|complete|failed|cancelled`, error is a catalogued diagnostic |
| `hub.download.cancel` | `{job:id}` | `{cancelling:id}`; removes partial data before commit |
| `hub.releases.check` | `{}` | `{checking:true}`; refresh per-page ETags and signed cache |
| `hub.releases.configure` | `{repo?,channel?}` | Persist update.json, clear prior feed and check; rejects changes during active work; ARA_RELEASE_REPO overrides repo |
| `hub.updates` | `{channel?}` | Same feed snapshot, filtered to Hub releases; no process restart |
| `hub.update.prepare` | `{version,hubVersion?}` | Hub download job; complete result `{archive,manifest,signature}` for independent external-updater verification |
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

`hub.changed {kind:"closed",project:id,revision}` is emitted when the authority
process exits (Hub Close, window X, crash or kill). Windows observers block on
process/cancellation kernel objects; discovery-file polling is not an exit signal.
Status reads use authenticated cached metadata plus process liveness. Service
restart rediscovers live authorities once; late exits cannot close a replacement.
Activity uses persistent connections at 2 s intervals while subscribed/open;
unchanged snapshots emit nothing. GUI clients run all calls off their input thread.

`hub.changed` kinds `releases` and `progress` invalidate release/job snapshots.
`hub.engines` adds `managed:bool` (external registrations are unregister-only).
Offline/rate-limited feeds carry a clear status; signatures are reverified on cache
reads. Install roots: LOCALAPPDATA/Ara/Engines/version. Cancelled/tampered extraction
never registers. Imported project pins may name a missing semantic version.

Render projects, Create, lifecycle, installed engines and activity. Errors are
standard RpcError diagnostics: show messages and fixes. UI: `crates/ara-hub`.

`hub.resolve {project}` returns a card. `hub.catalog {project?}` returns the running project catalog or the core method
catalog without opening any project. `hub.call {project,method,params}` opens a
registered project headless on demand and forwards to its authority. Each Hub
socket/project pair has a distinct downstream socket and session attribution.
MCP responses include `notices` from participant-specific conflict events;
normal replica op streams are filtered so idle agents do not accumulate them.
`hub.shutdown {onlyIfIdle?:true}` returns `{stopping:false,reason:"busy"}` if projects
or downloads are running. Build-mismatched clients use this before replacement;
legacy services without a build field are checked for open projects first.
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
CreateProcessW without handle inheritance to preserve captured CLI/MCP EOF. Services
use their config directory as CWD, and project children use the project root, so
neither keeps the Hub installation locked during a self-update.
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

Every test/capture/manual check uses a scratch ARA_CONFIG_DIR under target/: the
tracked `.cargo/config.toml` sets `target/ara-config` for `cargo test`/`cargo run`
(an explicit value wins). Config-isolation tests fail if it resolves elsewhere. Test cleanup
closes projects, shuts down the owned service and waits for discovery removal.
Do not stop or alter another user's/service's profile.

`CARGO_BUILD_JOBS=4 cargo test -p ara-hub-service` covers durable registry/templates and release jobs.
`cargo test -p ara --test hub` runs real user Hub + global stdio MCP + two project
daemons, concurrent clients, registry metadata, subscriptions, and install output.
`cargo test -p ara --test engine` retains per-project MCP/HTTP coverage.
`tests/process_exit.rs` uses windowless editor-authority processes to cover all
four exit paths, including stale discovery after crash/kill. Watcher cancellation
joins its thread and never terminates the observed editor when the Hub stops.
`cargo clippy -p ara-hub-service --all-targets -- -D warnings`; scoped rustfmt.
No new root workspace dependency: member discovery is already `crates/*`.
