# ara-ui

Purpose: one retained native UI runtime for the editor, game screens and world panels.
Depends on ara-ui-document, ara-ui-text, Taffy, Parley and AccessKit; never ara-gpu.

Public API:
- `Ui::parse` for games; `Ui::editor` for D18 editor themes and raw-color diagnostics.
- `Ui::frame` runs restyle, Taffy/Parley measurement and retained paint emission.
- `Ui::reconcile(markup) -> Reconciled` updates the retained tree from regenerated markup:
  nodes matched by ID+tag keep shaped text, Taffy node, style, hover/focus/press, visibility,
  paint offset and live widget state (an input keeps caret/undo while its `text` is unchanged
  or equals its value). Hosts rebuild markup on model changes and reconcile; never re-create
  `Ui` per change. `Ui::set_stylesheet` swaps CSS (no-op when equal).
- `Ui::reconcile_parts(skeleton, parts)`: parts (host ID, markup) replace the skeleton
  element with that ID. An unchanged part keeps its subtree wholesale (runtime rows and
  widget state too; nodes only change index, `Ui::moved_index`); a no-op touches nothing,
  and changed parts that keep their shape (IDs, tags, nesting) update in place.
  `VirtualView::remap` follows kept rows, `VirtualView::fit` resizes a kept pool.
- Hidden subtrees (`set_visible(false)`, `display:none`) stay mounted with their state but
  leave layout, paint, hit testing and focus; `Ui::shown(index)` says whether a node is
  (its rect is stale otherwise).
- `Ui::find` / `by_id` resolve authored/model-key identities through a lazily rebuilt index;
  `Work` exposes actual work counters (`+=` sums frames).
- `theme::EDITOR_CSS`, `EditorTheme::{Dark,Light}`, `Ui::switch_theme` use the canonical root
  assets; a switch swaps tokens and restyles once (no relayout/reshape); the same theme is free.
- `Sheet::compute` memoizes cascades by tag/class/inline style/state (and ID when an ID rule
  names it); `Sheet::set_tokens` swaps theme tokens and clears the memo.
- `paint::DisplayList` owns renderer-neutral commands and a contiguous positioned-glyph arena.
- `virtualize::{Rows,VirtualList,VirtualTree,VirtualTable,VirtualView}` retains only viewport rows.
- `input::{InputState,InputEvent}` owns per-player screen/modal focus, capture and drag/drop.
- `Ui::cursor` resolves widget defaults and `cursor`/`active-cursor` markup hints.
- `<scroll>` clips overflowing content and paints draggable, pageable scrollbars.
  `scroll_position`/`scroll_to` expose offsets; `set_virtual_extent` publishes the
  full model extent without mounting rows. Virtual pools own vertical translation;
  the widget applies horizontal translation. Hosts synchronize pools after input.
- `layers::LayerStack` owns tooltip/popup/modal visibility and dismissal/focus restoration.
- `docking::{DockLayout,DockNode,DockDrop,Floating}` moves tabs, creates splits, tears panels
  off into floating windows (`float`, `close`, `set_rect`), persists and resets layouts.
  Panel/area IDs are unique across every window; emptied areas collapse and emptied
  windows close. `set_collapsed` shows an area as just its tab strip (`collapsed_by_default`
  for a default's). Files without floats or collapsed areas keep the bare `DockNode` format.
- Change `Ui::size` only through `Ui::resize`: it marks the layout dirty (a direct write
  keeps the old size's layout until some node changes).
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

Tests: `CARGO_BUILD_JOBS=2 cargo test -p ara-ui`.
Also run `cargo clippy -p ara-ui --all-targets -- -D warnings` and package rustfmt check.
Rendering goldens and release measurements live in ara-ui-render.

Numeric editors can select an explicit font-family and text-align:right. These
properties retain shaping correctly; focused fields keep their caret origin.
Missing success/info surface tokens derive from the canonical palette colors.

DisplayList supports analytic Line strokes and ScalableGlyphRun labels for graph
canvases. Per-edge padding properties preserve inspector gutters at narrow widths.

`color: inherit` (opt-in) takes the parent's computed color, and children restyle when
it changes, so a label follows its button's hover/selected/disabled state. `<image
tint='true'>` multiplies a white image (icons) by its element's computed color;
`Draw::Image.tint` is white for ordinary images.
`paint_order`/`paint_starts` give each painted node's first display-list index, so
hosts can splice paint-time decorations beneath later layers such as menus.
Every node defaults to `position: relative` (taffy 0.15's default is static), so an
absolute child's `right`/`bottom` resolve against its parent, where paint puts it.
Only a focused input scrolls to its caret; unfocused inputs draw from their start, and
a click on one first puts the caret there so the hit test matches what was drawn.
