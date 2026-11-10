# ara-hub

Ara Hub (D25), `ara-hub`: projects, templates/samples, pinned engine installs, agent
onboarding and activity. Built on stillkit (D31) with Ara's look from `ara-kit`: one
CPU-drawn app window that redraws only what changed and sleeps with no timer while idle. No
GPU device, SDL or ara-ui. Design: `docs/design/hub.md`.

All actions use Hub JSON-RPC; its contract is `../ara-hub-service/AGENTS.md`.

## Layout

- `model/`: `Hub`, the whole view-model. `intent(Intent)` for people, `event(connection::Event)`
  for the service; it queues `Call`s (`take_calls`) and `Effect`s (`take_effects`: folder
  pickers, Finder/Explorer, clipboard, applying an update). No window or socket, so tests
  drive exactly what the window drives. `create.rs` is the Create form; `wire.rs` the service
  result types; `engines.rs` finds installed builds; `text.rs` formatting.
- `connection.rs`: the service on its own thread (`connect_or_start`, `hub.subscribe`
  heartbeat, reconnect with backoff, `Offline` names `hub-start.log`). Its wake hook is
  stillkit's `Waker`.
- `ui/`: the interface. `HubApp` is the runtime state (the `Hub`, the `Look`, images, the
  palette, the open dropdown); `view` builds every element from it. Handlers call
  `HubApp::intent`. Pages: `projects` (cover-art cards or a dense list; one primary action per
  project, the rest in its menu: ⋯, right click or the Menu key; double-click/Enter opens,
  F2 renames in place), `create` (full window; dry-run problems under their field),
  `installs`, `agents` (live activity first when anyone is attached), `learn`, `settings`.
  `shell` is the title bar (⌘K launcher, update pill, theme), sidebar, page frame, banners,
  toasts and code blocks; `dialogs` every decision flow; `palette` the command palette
  (fuzzy, grouped, keyboard-only); `images` decodes thumbnails off the UI thread, generates
  covers (`ara_kit::art`) and rasterizes brand SVGs.
- `native/`: the host loop (`mod.rs`): one `Surface` with a compact caption (traffic lights
  centred on macOS; drawn controls registered as native chrome elsewhere), the runtime, the
  service, effects, preference writes (`prefs.rs`, off-thread), pictures and snippets. Idle
  waits end at the next relative-time minute, a toast expiry or the four-hourly update
  check. `os.rs`: native pickers (`stillkit::dialog`), Finder/Explorer, install snippets,
  local docs. `startup.rs` the startup probe; `rehearsal.rs` the release rehearsal.
- `capture.rs`: `Capture`, a headless Hub window (the real view on a stillkit runtime with
  fixture models swapped in), and `ara-hub --capture DIR`: every page, menu, palette and
  dialog flow in both themes, with interaction assertions (`capture/*.rs`).

## Behaviour worth knowing

- Shortcuts (⌘ is Ctrl elsewhere): ⌘K palette, ⌘N new project, ⌘O add existing, ⌘1-4 pages,
  ⌘, settings, / search, Esc leaves Create; folders dropped on the window register.
- Notifications have one place each (`docs/design/hub.md`): errors open dialogs, the quiet
  update pill opens release notes, successes are toasts (Undo for removal). Every banner
  has Dismiss; toasts expire (8 s, Undo 20 s), errors wait.
- Create validation is the service's: every edit sends `hub.create {dryRun:true}`;
  diagnostics carry `location.path` = `name|path|template|engineVersion`.
- Lifecycle calls show progress chips on the card; ARA1914 opens the migration preview,
  ARA1915 the engine-layout choice. Other open failures link `.ara/hub-process.log`.
- `model/updates.rs`: signed releases. Ask first is the default download policy; restart is
  always explicit. Standalone `hub.json` builds bootstrap the immutable service into the
  user's Ara services folder; `ara-hub-updater` swaps with rollback. Captures never
  bootstrap, connect or check releases.
- Prefs (theme, sort, layout, agent client) persist to `<config>/hub-ui.json`.

## Tests

Set `CARGO_BUILD_JOBS=2` on shared machines and an explicit scratch profile for anything that
can start a service: `ARA_CONFIG_DIR=<repo>/target/hub-checks/<id>/config` (and
`ARA_CRASH_DIR` under `target/` when running the binary directly). Checkout binaries reject
the real user profile.

- `cargo test -p ara-hub`: `tests/hub.rs` drives the real view headlessly (every page in both
  themes at three sizes; primary actions per state; menu; rename by F2/Enter/Escape; search;
  Create back/Escape; removal Undo; dialog confirm/cancel/Escape; the palette by keyboard;
  theme; input during a slow service; an idle window draws nothing). `tests/updates.rs` the
  release model. `cargo test -p ara --test hub_ui` drives the model against a real service.
- Captures: `target/debug/ara-hub --capture target/hub-captures --size 1440x900 --dpi 1` and
  `--size 2560x1440 --dpi 2`. Look at every PNG, both themes.
- `cargo clippy -p ara-hub --all-targets -- -D warnings`.

## Gotchas

- Components come from `ara_kit::ui` (keyed by the caller); add generic ones to stillkit or
  stillkit-kit, Ara-specific ones to `ara-kit`, never ad-hoc styling here.
- An open menu takes the next outside click to close itself, as native menus do.
- Never point a dev Hub at the default config dir: starting the service from a dev `ara`
  re-registers `0.0.1-alpha` to that dev build.
