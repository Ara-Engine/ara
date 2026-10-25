# ara-hub

Ara Hub (D25): the native launcher, `ara-hub.exe`. Projects, create from templates
and the real-art samples, engine installs with per-project pins, agent onboarding
and live activity. Runs on the editor's UI stack: `ara-ui` retained UI, SDL3 via
`ara-platform`, the editor's Vulkan adapter `ara-editor-gpu` on a plain
`ara_gpu::vulkan::Context` (no scene renderer), the same theme tokens, fonts and
Lucide icons, DWM-styled native frame (D23), high-DPI.

Everything it does goes through the user Hub service's JSON-RPC API; the wire
contract is `crates/ara-hub-service/AGENTS.md`.

## Layout

- `model/`: `Hub`, the whole UI state. `intent(Intent)` for people,
  `event(connection::Event)` for the service; it queues `Call`s (`take_calls`)
  and native `Effect`s (`take_effects`: folder pickers, Explorer, clipboard).
  No window, GPU or socket, so tests drive exactly what the window drives.
  `create.rs` is the Create form; `wire.rs` the service result types (unknown
  fields ignored); `engines.rs` finds installed builds; `text.rs` formatting.
- `connection.rs`: the service on its own thread. `connect_or_start` with the
  `ara` beside `ara-hub` (`--ara` overrides), `hub.subscribe` before `Connected`
  (the model then refreshes every list), `hub.changed` → `Event::Changed`, a 2 s
  `hub.subscribe` heartbeat, reconnect with backoff, `Offline` names `hub-start.log`.
- `view/`: markup per page from the model (`page`), `hub.css` (theme tokens only,
  D18), and `Page::bindings` mapping each control's stable ID to its `Intent`.
- `native/`: window/GPU host and loop (`mod.rs`), retained `App` (`app.rs`: rebuild,
  input routing, keys), `input.rs` (SDL text/IME, clipboard, folder drops),
  `frame.rs` (DWM colors), `images.rs` (icons, wordmark, previews decoded off
  thread, center-cropped to 16:9), `os.rs` (pickers, Explorer, install snippets,
  local docs).
- `capture.rs`: `ara-hub --capture DIR` seeds a fixed fixture through the model's
  reply paths and writes every page, dark and light, from the real GPU target.
  No window opens: a headless Vulkan device drives the same `NativeUi` adapter.

## Behavior worth knowing

- Equal markup replies do no UI work. Changed markup uses `Ui::reconcile` on the
  same tree, retaining shaped text, layouts, widget state and input identity.
  Scroll offsets clamp after layout; explicit focus requests and hover update then.
- Theme changes call `switch_theme` once to swap palette tokens. Initial inherited
  colors resolve in the runtime; `Ui::editor` is used only for window creation.
- `App::frame` consumes a retained presentation request: rebuilding must present
  even if its layout work was already consumed. Pointer activations apply before
  the next queued event. Worker results wake SDL; idle waits follow clock/toast deadlines.
- Notifications have one place each (docs/design/hub.md): errors are inline banners,
  a Hub update is the sidebar card (in full on Settings), results are toasts.
- Every toast/banner has Dismiss. Informational toasts expire after 8 s (Undo: 20 s);
  errors remain until dismissed. Preference writes are coalesced off the UI thread.
- Create validation is the service's: every edit sends `hub.create {dryRun:true}`;
  diagnostics carry `location.path` = `name|path|template|engineVersion` and render
  under that field. One dry run in flight; a stale reply re-checks.
- Lifecycle calls show progress (`Opening…`), never approval (D24). Open failures
  link `.ara/hub-process.log`; service startup failures link `hub-start.log`.
- Remove keeps files and its toast offers Undo (re-register). Diagnostic fixes run
  only `hub.*` `Call` fixes; `Cli` fixes are copied as `ara …`.
- First run of an installed build (an `engine.json` beside the exe) registers that
  build with `hub.engine.register` if the service lacks it. Dev builds don't.
- Folder drops on the window register the folder (`Intent::Register`).
- Prefs (theme, sort, layout, agent client) persist to `<config>/hub-ui.json`.
- `model/updates.rs`: authenticated release metadata and jobs from the service.
  Installs shows published versions, progress/cancel and managed Uninstall;
  Settings configures channel/repository and checks for updates. Native checks
  at startup and every four hours; standalone Hubs prepare updates automatically.
- Standalone `hub.json` builds bootstrap the immutable service into
  LOCALAPPDATA/Ara/Services/version. `ara-hub-updater` runs from config/updater,
  waits for this GUI only, verifies again and swaps with rollback. Health is
  acknowledged after the first rendered GUI frame. Captures never bootstrap,
  connect or check releases. Engine-bundled Hubs cannot replace their engine dir.
- Release builds use the Windows GUI subsystem; fatal startup errors show a message box.

## Copied from ara-editor (extract later)

`native/input.rs` (text watch, clipboard) and `native/frame.rs` (DWM style) are
minimal copies of `ara-editor/src/native/`; fonts are embedded from
`ara-editor/assets/fonts`. Move them into a shared native-UI crate once the
editor's panel rebuild lands.

## Tests

Set `CARGO_BUILD_JOBS=4`, `VULKAN_SDK` and an explicit scratch profile for every
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
- Verification uses offscreen captures only. Stop/join the connection worker
  before shutting down a test service; await removal of its discovery file.
  Never leave a service/updater you started running after a check.
- `cargo clippy -p ara-hub --all-targets -- -D warnings`.
- `tests/responsiveness.rs` covers presentation, idle, dismissal and queued clicks;
  `tests/async_rpc.rs` holds a service reply while input still completes.
  Opt-in CPU timings: set ARA_HUB_TIMINGS to an absolute target/.../timings.json,
  then `cargo test -p ara-hub --test responsiveness measure_hub_frames -- --ignored`.
  Captures also inject menu, rename and dismiss clicks and save before/after images.

## Gotchas

- `ara_ui` CSS: one class/type/ID per selector, flex/grid with `fr` tracks only,
  no font weight, colors only through `var(--token)`. Grid cards need
  `min-width:0px` or long names widen their track.
- The texture heap is 256 images (icons, wordmarks, atlases, previews).
- Never point a dev Hub at the default config dir: starting the service from a
  dev `ara` re-registers `0.0.1-alpha` to that dev build.
