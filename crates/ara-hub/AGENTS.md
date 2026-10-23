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

- Every model change rebuilds the `Ui` from markup. `App::rebuild` carries field
  `TextInput`s (caret, selection, undo) and scroll offsets across by stable ID,
  then restores focus and re-derives hover.
- `App::rebuild` always calls `switch_theme`, even for Dark: a fresh `Ui` does not
  resolve `color: inherit` until something restyles it (ara-ui quirk; without it
  icons in primary buttons stay muted).
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
- Release builds use the Windows GUI subsystem; fatal startup errors show a message box.

## Copied from ara-editor (extract later)

`native/input.rs` (text watch, clipboard) and `native/frame.rs` (DWM style) are
minimal copies of `ara-editor/src/native/`; fonts are embedded from
`ara-editor/assets/fonts`. Move them into a shared native-UI crate once the
editor's panel rebuild lands.

## Tests

Set `CARGO_BUILD_JOBS=4` and `VULKAN_SDK`.

- `cargo test -p ara-hub`: `tests/views.rs` lays out every page in both themes at
  1440x900 and 1280x720 (no GPU); `tests/service.rs` drives the model against a
  real Hub service in `target/hub-ui-tests/<scratch>/config` (create with inline
  validation, open headless, badge, close, remove, undo). It runs
  `cargo build -p ara --bin ara` first so the service matches the checkout.
- Captures (GPU, windowless; debug builds need the validation layers):
  `VK_IMPLICIT_LAYER_PATH=<empty dir> VK_LAYER_PATH=$VULKAN_SDK/Bin`, then
  `target/debug/ara-hub --capture target/hub-captures --size 1440x900 --dpi 1`
  and `--size 2560x1440 --dpi 2`. Look at every PNG.
- Try it without touching your real registry:
  `ara-hub --config-dir target/hub-dev-config` (this one opens a window).
- `cargo clippy -p ara-hub --all-targets -- -D warnings`.

## Gotchas

- `ara_ui` CSS: one class/type/ID per selector, flex/grid with `fr` tracks only,
  no font weight, colors only through `var(--token)`. Grid cards need
  `min-width:0px` or long names widen their track.
- The texture heap is 256 images (icons, wordmarks, atlases, previews).
- Never point a dev Hub at the default config dir: starting the service from a
  dev `ara` re-registers `0.0.1-alpha` to that dev build.
