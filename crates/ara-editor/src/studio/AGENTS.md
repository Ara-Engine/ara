# studio: Ara's editor on stillkit (D34)

The editor window: one stillkit `Runtime<Studio>` drawn on the CPU and redrawn only where
something changed, with the Scene view as a native GPU layer under a hole. It is a client
of `Editor` like any agent: navigation goes through `editor.*` actions and content through
`cmd.apply` (`Studio::act`, `Studio::apply`), so every person's action has an agent
equivalent. `--legacy-ui` opens the old `shell` window.

## Files

- `mod.rs`: the host loop. It pumps window events (`pump`), the protocol server and
  `Editor::tick`, keeps the renderer in sync (`SceneSync`), renders and composites the
  Scene when it is dirty, resolves picks, polls Assets and the diagnostics bus, applies
  `Local::focus` requests, and sleeps until the next timer (idle costs nothing).
- `state.rs`: `Studio` (editor, look, resolved scene, window-only `Local`, size).
- `game.rs`: the Game view's own layer (`GAME_LAYER`): the gameplay camera's preview outside
  Play (`app::game_preview` through its `GameHost` seam), the embedded player's frames during
  Play (`app::game::player::PlayerView`; `play.start` calls `Editor::embed_play`). A click
  sends keys and the mouse to the player (VK to SDL scancodes); Esc gives them back.
- `components.rs`: Add Component (Unity's picker): categories from each component's schema
  module, search across all, recent first, arrows/Enter/Left, the description below.
- `asset_inspector.rs`: an asset in the Inspector (Unity) and the Import tab: preview,
  importer settings (`shell::import_batch`, one `import.configure` step each), Reimport,
  Open, Uses / Used by (`shell::import_panel::links`).
- `lighting.rs`: everything that lights the scene (`shell::lighting::{model, authoring}`).
- `profiler.rs`: Record/Stop (`profile.start`), a frame strip against the budget, the
  frame's thread lanes, scopes by self time, GPU passes. Each loop pass is a profiled frame
  while recording (`trace::Pass::phase` scopes); `Studio::profile` is a copy refreshed 4 Hz.
  Tool pages (Profiler, Material, VFX, Animation) dock in the centre: no floating windows yet.
- `layer.rs`: a native layer with its own GPU UI adapter (`Layer::draw` of a display list);
  the Game view and the graph canvas each own one.
- `graph.rs`: the Material and Animation pages: the shared node canvas
  (`crate::graph::Canvas`) in `graph::PAGES`' layers; gestures become document commands
  (`material.connect`, `graph.node.move`; controllers through
  `shell::animation_panel::{connect, edit}`). Unreal's habits: generated input defaults
  draw inline on their pins (`graph::is_default`; click to type), the node search opens
  at a right-click or dropped wire and lists only what fits it, C makes a comment box,
  ⌘D duplicates (`editor.material.node.duplicate`), Delete removes; Details on the right.
- `vfx.rs`: the VFX page, laid out as Niagara's: preview and timeline (left), System
  Overview cards per emitter with coloured stages (middle), the picked module's Selection
  (right); `shell::vfx_panel::module_edit`, preview from `asset_tools::vfx_stream`.
- `asset_create.rs`: Add / `assets.create.*`: folder, scene, material, effect, controller,
  script, at the open folder's authored path (never its storage path), opened where edited.
- Assets (`panels.rs`): back/forward, Add, type filters across the project (Unreal's
  filter bar), list or three tile sizes, a type strip under tiles.
- `compositor.rs`: the renderer's image plus `shell::overlay` (grid, outlines by entity,
  gizmo), drawn by `ara-editor-gpu`'s `NativeUi` and presented into the layer
  (`create_layer_present`). Picking goes through the same channel map.
- `chrome.rs`: root view, title bar (menus from `shell::commands::menus`, Play,
  scene crumb, ⌘K launcher), status bar, palette, context menus.
- `dock.rs`: `editor.state.layout` as rounded panels, tab strips and splitters.
- `scene.rs`: the Scene element and toolbar. RMB look + WASD fly (key-ups from the host),
  MMB pan, Alt+LMB orbit, wheel dolly, click to pick, right-click menu, Shift+A at pointer.
- `gizmo.rs`: handle hover/drag mapped to an editor gesture; release commits one step.
- `hierarchy.rs`: scene root row, virtual tree, rename, visibility/lock, WAI tree keyboard,
  drag to reparent (quarters: before / inside / after) through `shell::drag::content::hierarchy`.
- `inspector.rs`: fields from `crate::inspector::fields`; label scrubs (8 px a step,
  Shift ×0.1, Alt ×10), typed units, vectors, enums, colours; field menu (Reset, Copy
  Value, Copy Path); component ⋯ (Fold Others, Remove); Add Component.
- `references.rs`: asset/entity object fields (searchable picker, None first; `Asset::fits`).
  `compound.rs`: lists, maps and nested structs fold open in place; every element edit is
  `editor.property.nested.edit` at a JSON pointer (`path#pointer` targets).
- `create.rs`: the Create browser over `CATALOG` (registry `create.*` ids; an entry shows
  when the schema has its components). Recent, search, Enter, Shift+A placement.
- `panels.rs`: Assets (folder tree + grid, thumbnails from `asset_tools::thumbnails` via
  `state::Previews`, models/prefabs drag into the Scene), Console, Problems (one-click
  typed fixes via `shell::input::apply_fix`); other panels say they are coming.
- `preferences.rs`: Preferences / Project Settings generated from `crate::settings::DEFS`.
- `agents.rs`: Agents feed (presence, turns, Undo Turn) and History.
- `commands.rs`: which registry commands the studio runs (`ported`), `run`, keymap,
  macOS shortcut glyphs. Menus hide (context) or disable (title) unported commands.

## Rules

- Never write documents directly: `Studio::apply` (one undo step) or a registry command.
- A command that opens a field sets `local.focus`; handlers with a `Tx` focus directly.
  Copies set `local.copy`; the host writes it to the system clipboard.
- Window-only state lives in `Local`; anything agents should see goes to `EditorState`.
- Visual review: `cargo test -p ara-editor --lib studio::tests::captures -- --ignored`
  writes both themes (panels, context menu, Create) to `target/studio-captures/`.
- Spacing: 3 px gutters, 26 px tab strips, 20 px tree and field rows, 24 px dense
  controls. Fields set `focus_ring(false)` where their well shows focus.
- One flat look (D34): no lines and no elevation. `style::chosen` marks the chosen row,
  folder, tile, item or tool in the Raised tone; `style::float` makes popups the Float
  material; fields are Wells. Scene controls use the panels' strip tone, not glass.

## Tests

`studio/tests.rs` drives the runtime headless over a real `Editor` on a scratch project:
gizmo drag (one transaction) and Escape, Hierarchy click and arrows, context menu, Create
search + Enter, Assets menus, material search/inline values/comments/duplicate, the VFX
Selection. `cargo test -p ara-editor --lib studio::`. `rt.touch` changes state without
re-running the view; use `rt.update` (or settle) before finding what it should show.

## Gaps

The Scene layer presents on macOS only (Windows needs the Vulkan HWND path). The material
preview sphere needs slangc (the Vulkan SDK). Tool pages dock; no floating windows yet.
