# ara-ui

Purpose: one retained native UI runtime for the editor, game screens and world panels.
Depends on ara-ui-document, ara-ui-text, Taffy, Parley and AccessKit; never ara-gpu.

Public API:
- `Ui::parse` for games; `Ui::editor` for D18 editor themes and raw-color diagnostics.
- `Ui::frame` runs restyle, Taffy/Parley measurement and retained paint emission.
- `Ui::find` / `by_id` resolve authored/model-key identities; `Work` exposes actual work counters.
- `theme::EDITOR_CSS`, `EditorTheme::{Dark,Light}`, `Ui::switch_theme` use the canonical root assets.
- `paint::DisplayList` owns renderer-neutral commands and a contiguous positioned-glyph arena.
- `virtualize::{Rows,VirtualList,VirtualTree,VirtualTable,VirtualView}` retains only viewport rows.
- `input::{InputState,InputEvent}` owns per-player screen/modal focus, capture and drag/drop.
- `layers::LayerStack` owns tooltip/popup/modal visibility and dismissal/focus restoration.
- `docking::{DockLayout,DockNode,DockDrop}` moves tabs, creates splits, persists and resets layouts.
- `binding::{BindingKey<T>,ReadBinding<T>,Bindings<T>}` initializes subscriptions and pushes changed fields.
- `Ui::drain_actions` yields named host actions; the host dispatches these at its simulation boundary.
- `accessibility::accessibility` produces an actual AccessKit tree; `Ui::inspect` produces agent JSON.
- `InputState::focus_graph` reports only controls reachable within the active input scope.
- Style provenance preserves full selectors and declaration value spans for capture/designer inspection.

Integration:
1. Create `TextSystem` with the application's licensed fallback packs.
2. Parse markup/styles, mount virtual views into empty scroll nodes, and call `frame` once.
3. Push an input screen scope. Translate SDL3 events/IME/clipboard through the typed seams.
4. Route overlay input through `LayerStack::event` so dismissals also hide layers.
5. Apply model notifications, input actions and scroll updates, then call `frame` again.
6. Render `display` through ara-ui-render and update the platform AccessKit adapter.
7. Send authoring changes through source `Command` -> `SourceOp`, wrapped in host transactions.

Widget rules:
- Fields are single-line, grapheme-safe, selection/caret/clipboard/IME/undo controls.
- Toggle, slider, scroll, splitter, choice/tabs/menu/dropdown state is retained natively.
- A tabs control's direct children are pages; `select` activates one page. The host supplies tab labels/actions.
- Dropdown/menu children are hidden until activated; independent anchored menus use `LayerStack`.
- Splitters adjust adjacent sibling flex weights; persist their action through `DockLayout::resize_split`.
- Dock overlays resolve their drop location to `DockDrop::Tab` or `DockDrop::Split`.
- Virtual rows emit `ActivateRow`; the host maps stable row identity to its model key/expand action.
- Source tree expansion is notification-driven. A tree scroll visits only the viewport projection.
- Input state follows model keys, never pool slots. Recycled rows clear hover/focus/press state.
- Clips affect paint, hit testing and focus; rounded clips require renderer stencil/mask intersection.
- Runtime visibility remains independent of source styles/theme switching.
- Utility classes are not compiled; arbitrary classes are semantic stylesheet hooks.
- Layout supports Flexbox/Grid, min/max sizes, alignment, absolute overlays and translations.
- `flex-basis` supports dock weights; `white-space:nowrap` prevents short chrome
  labels wrapping. `InputState::blur` hands text focus back to native viewport input.
- General browser CSS, selector combinators and arbitrary transforms are diagnosed as unsupported.

Tests: `CARGO_BUILD_JOBS=4 cargo test -p ara-ui`.
Also run `cargo clippy -p ara-ui --all-targets -- -D warnings` and package rustfmt check.
Rendering goldens and release measurements live in ara-ui-render.
