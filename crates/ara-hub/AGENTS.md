# ara-hub

Ara Hub (D25), `ara-hub.exe`: projects, templates/samples, pinned engine installs,
agent onboarding and activity. Uses `ara-ui`, SDL3/`ara-platform`, `ara-editor-gpu`
on `ara_gpu::vulkan::Context` (no scene renderer), shared themes/fonts/Lucide,
and a high-DPI DWM-styled native frame (D23).

All actions use Hub JSON-RPC; its contract is `../ara-hub-service/AGENTS.md`.

Icons: `build.rs` embeds `assets/brand/icons/hub/ara-hub.rc` into both Hub and
`ara-hub-updater`; the native Hub window explicitly sets the Hub 64 px PNG.
The sidebar pairs the new Hub SVG icon with native Ara text. Engine cards and
installation dialogs use the engine SVG family; update dialogs use the Hub icon.

## Layout

- `model/`: `Hub`, the whole UI state. `intent(Intent)` for people,
  `event(connection::Event)` for the service; it queues `Call`s (`take_calls`)
  and native `Effect`s (`take_effects`: folder pickers, Explorer, clipboard).
  No window, GPU or socket, so tests drive exactly what the window drives.
  `create.rs` is the Create form; `wire.rs` the service result types (unknown
  fields ignored); `engines.rs` finds installed builds; `text.rs` formatting.
- Create is a dedicated full-window page, opened by New project or a template;
  it has no sidebar entry. Back, Esc and mouse Back return to the opener. Native
  page state restores its scroll/focus, while Projects search and preferences stay
  in the model. Nested Select popups and dialogs consume Back/Esc first.
  Successful creation queues the existing Open RPC for the new project.
- `connection.rs`: the service on its own thread. `connect_or_start` with the
  `ara` beside `ara-hub` (`--ara` overrides), `hub.subscribe` before `Connected`
  (the model then refreshes every list), `hub.changed` → `Event::Changed`, a 2 s
  `hub.subscribe` heartbeat, reconnect with backoff, `Offline` names `hub-start.log`.
  Native startup uses `start_preparing_with_wake` before device creation: bootstrap
  hashing/copying and the handshake overlap GPU initialization. `Prepared` supplies
  the verified CLI for native integrations; preparation failures are non-fatal/retried.
- `view/`: markup per page from the model (`page`), `hub.css` (theme tokens only,
  D18), and `Page::bindings` mapping each control's stable ID to its `Intent`.
- `view/dialogs.rs`: confirmations, errors, release notes, transfer progress and kit
  shared searchable Select choices; native input uses the kit layer stack for dialogs.
  Install/repair dialogs request read-only preflight space summaries; explicit Install current engine uses a verified same-version replacement lane. Technical source
  settings stay under Advanced. Repair jobs retain engine pins.
  Uninstall first inspects destination/size in a cancellable read-only job; only the
  subsequent explicit confirmation can start removal.
- `native/`: window/GPU host and loop (`mod.rs`), retained `App` (`app.rs`: rebuild,
  input routing, keys), `input.rs` (SDL text/IME, clipboard, folder drops),
  `selects.rs` retains typed choices across replies and routes native search/IME;
  `frame.rs` (DWM colors), `images.rs` (icons and previews decoded off
  thread, center-cropped to 16:9), `os.rs` (pickers, Explorer, install snippets,
  local docs).
- `capture.rs`: `ara-hub --capture DIR` seeds a fixed fixture through the model's
  reply paths and writes every page, dark and light, from the real GPU target.
  No window opens: a headless Vulkan device drives the same `NativeUi` adapter.
  `capture/flows.rs` adds update/install/error states; `capture/migration.rs` adds migration consent/progress in both themes.

## Behavior worth knowing

The opt-in `rehearsal` feature adds `--rehearse-update REPORT`; with the test-only
loopback feed it drives the actual update model and external updater without a
window. Updated Hub health renders offscreen and waits for the updater's ack.
This feature stays off in shipping builds; see ara-release's rehearsal command.

- Equal markup replies do no UI work. Changed markup uses `Ui::reconcile` on the
  same tree, retaining shaped text, layouts, widget state and input identity.
  Scroll offsets clamp after layout; explicit focus requests and hover update then.
- Theme changes call `switch_theme` once to swap palette tokens. Initial inherited
  colors resolve in the runtime; `Ui::editor` is used only for window creation.
- Built-in icons, brand marks and Starter art decode/upload on first visible use,
  then stay resident across pages/themes. DPI changes invalidate that image set.
  MCP snippet helper processes start only on opening Agents, after bootstrap is ready.
- Agent instructions quote the verified absolute CLI path (including spaces/apostrophes).
  Hub copies configs/commands; it does not write or own the clients' config files.
- `App::frame` consumes a retained presentation request: rebuilding must present
  even if its layout work was already consumed. Pointer activations apply before
  the next queued event. Worker results wake SDL; idle waits follow clock/toast deadlines.
- Notifications have one place each (docs/design/hub.md): errors use focused dialogs,
  a quiet update indicator opens release notes, and successful results use toasts.
- Every toast/banner has Dismiss. Informational toasts expire after 8 s (Undo: 20 s);
  errors remain until dismissed. Preference writes are coalesced off the UI thread.
- Create validation is the service's: every edit sends `hub.create {dryRun:true}`;
  diagnostics carry `location.path` = `name|path|template|engineVersion` and render
  under that field. One dry run in flight; a stale reply re-checks.
- Lifecycle calls show progress (`Opening…`). ARA1914 opens a kit migration modal with move/tree preview and Migrate/Cancel. ARA1915 opens Use current engine (or Install current engine)/Cancel; signed installs recheck capabilities before a typed pin. Cancel stays closed; successful physical persistence resumes the original editor/headless mode. Other open failures link `.ara/hub-process.log`; service startup failures link `hub-start.log`.
- Remove keeps files and its toast offers Undo (re-register). Diagnostic fixes run
  only `hub.*` `Call` fixes; `Cli` fixes are copied as `ara …`.
- First run of an installed build (an `engine.json` beside the exe) registers that
  build with `hub.engine.register` if the service lacks it. Dev builds don't.
- Folder drops on the window register the folder (`Intent::Register`).
- Prefs (theme, sort, layout, agent client) persist to `<config>/hub-ui.json`.
- `model/updates.rs`: authenticated release metadata and jobs from the service.
  Installs shows published versions, phased progress/cancel and managed Uninstall;
  Settings configures channel/repository and checks for updates. Native checks
  at startup and every four hours. Ask first is the default download policy;
  Background downloads require opt-in. A prepared archive applies only from Restart and update. Later closes the dialog; Skip persists
  the exact version/build identity. Settings keeps skipped updates discoverable.
- Standalone `hub.json` builds bootstrap the immutable service into
  LOCALAPPDATA/Ara/Services/version/content-hash (service.json records the build).
  A build handshake replaces idle services; busy services stay connected with a notice.
  `ara-hub-updater` runs from config/updater,
  waits for this GUI only, verifies again and swaps with rollback. Health is
  acknowledged after the first rendered GUI frame. Captures never bootstrap,
  connect or check releases. Engine-bundled Hubs cannot replace their engine dir.
- Release builds use the Windows GUI subsystem; fatal startup errors show a message box.

`native/input.rs` and `native/frame.rs` copy the editor's text/clipboard and DWM
adapters; fonts embed from `ara-editor/assets/fonts`. Extract these into a shared
native-UI crate after the editor panel rebuild.

## Tests

Set `CARGO_BUILD_JOBS=2`, `VULKAN_SDK` and an explicit scratch profile for every
test, capture and manual check (PowerShell, from the workspace root):
`$env:ARA_CONFIG_DIR = (New-Item -ItemType Directory -Force "target/hub-checks/$([guid]::NewGuid())/config").FullName`.
The config-isolation test fails without this. Checkout binaries reject the real
user profile, even via --config-dir. Never register worktree builds there.

- `cargo test -p ara-hub`: `tests/views.rs` lays out every page in both themes at
  1440x900 and 1280x720 (no GPU). `cargo test -p ara --test hub_ui` drives the model against a
  real Hub service in `target/hub-ui-tests/<scratch>/config` (create with inline
  validation, open headless, badge, close, remove, undo). Cargo supplies the fresh
  CLI executable; the fixture never starts a nested build.
- Captures (GPU, windowless; debug builds need the validation layers):
  `VK_IMPLICIT_LAYER_PATH=<empty dir> VK_LAYER_PATH=$VULKAN_SDK/Bin`, then
  `target/debug/ara-hub --capture target/hub-captures --size 1440x900 --dpi 1`
  and `--size 2560x1440 --dpi 2`. Look at every PNG.
- Installed startup: `measure-startup.ps1 -Setup <Hub setup> -Directory <fresh target/dir>`
  through `ara-dev gpu`. Five fresh-profile/no-service launches paired with same-profile,
  running-service launches; OS caches are not flushed. `--startup-probe DIR` draws the
  live model offscreen, injects input while connecting and saves both milestone frames.
  Timings start immediately before Process.Start and exclude PNG encoding. Optional
  ARA_HUB_STARTUP_TRACE appends spans from GUI/service; probes never check the network.
- Verification uses offscreen captures only. Stop/join the connection worker
  before shutting down a test service; await removal of its discovery file.
  Never leave a service/updater you started running after a check.
- `cargo clippy -p ara-hub --all-targets -- -D warnings`.
- `tests/responsiveness.rs` covers presentation, idle, dismissal and queued clicks;
  `tests/async_rpc.rs` holds a service reply while input still completes.
  `tests/create_navigation.rs` drives Back/Esc/mouse return, viewport/search
  restoration, the shared engine Select, origin tracking and create-then-open.
  Opt-in CPU timings: set ARA_HUB_TIMINGS to an absolute target/.../timings.json,
  then `cargo test -p ara-hub --test responsiveness measure_hub_frames -- --ignored`.
  Captures also inject menu, rename and dismiss clicks and save before/after images.

## Gotchas

- `ara_ui` CSS: one class/type/ID per selector, flex/grid with `fr` tracks only,
  no font weight, colors only through `var(--token)`. Grid cards need
  `min-width:0px` or long names widen their track.
- The texture heap is 256 images (icons, atlases, previews).
- Never point a dev Hub at the default config dir: starting the service from a
  dev `ara` re-registers `0.0.1-alpha` to that dev build.
