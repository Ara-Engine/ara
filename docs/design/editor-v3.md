# The editor, v3: the studio

The studio is Ara's editor window rebuilt on stillkit (D31). The renderer is unchanged: the Scene view is a native GPU layer under the UI. The research behind it is `docs/research/editor-ui-v3/research.md`, and rule numbers below (I09, V05…) refer to it. The code lives in `crates/ara-editor/src/studio/`, and its `AGENTS.md` maps the files.

## Principles

- **Dense, not crowded.** Unity and Unreal show a lot at once without overwhelming: one spacing rhythm, quiet chrome, and colour only where it means something. The studio uses 4 px gutters, 30 px tab strips, 22 px tree and field rows, 24 px dense controls, 12–13 px text (V03, V05). Empty space is either given to the work (the Scene) or to the next action (Add Component under the last component).
- **One command identity.** Menus, the palette, context menus, shortcuts and agents all run registry commands (`shell::commands`). The studio runs what it has ported; title menus show the rest disabled with a reason, context menus hide them.
- **People and agents use the same surface.** The window calls `editor.*` and `cmd.apply` like an MCP client, so every human action has an agent equivalent, and agent edits appear in Agents with Undo Turn (D24).
- **Direct manipulation is immediate.** Scrubs, gizmo drags and camera moves follow the pointer with no easing (V11). Only short, explanatory transitions animate.
- **Focus shows once.** A field inside a well leaves focus to the well's border (`focus_ring(false)`).

## Layout

The default layout follows Unity: a full-height Hierarchy (16%), the Scene over Assets/Console/Problems/Agents, and a full-height Inspector wide enough for its labels (about 360 px at 1400 px). Panels are rounded surfaces 4 px apart, each with a tab strip. The title bar holds the menus, Play controls centred, the project and scene, and the ⌘K launcher. The status bar shows authority state, counts and the active shortcuts.

## Components

- **Scene view:** an orientation gizmo in the corner; click an axis to look from that side. Floating glass toolbars (transform tools, World/Local and Pivot/Centre, snapping; projection and Frame) over the picture, plus an fps chip.
  - Navigation: right-drag looks; held with WASD/QE it flies (Shift ×3). Middle-drag pans, Alt+left orbits, the wheel dollies.
  - Clicks: a click picks through the renderer's channel map, Shift toggles. A still right click picks, then opens the Entity or Viewport menu. Shift+A opens Create at the pointer.
  - Overlays: outlines by entity, the grid and the gizmo are composited on the GPU.
- **Gizmo:** hovering lights a handle, and a drag runs one editor gesture (along an axis, across a plane, around a ring). Release commits one undo step; Escape cancels.
- **Hierarchy:** a scene root row (name, count), then a virtual tree of 22 px rows with type icons.
  - Rows: inline rename (F2/Return), and visibility and lock toggles on hover.
  - Keyboard: Up/Down, Right/Left open or step, Home/End, Shift extends (I09). Search keeps matches and their ancestors (I15).
  - Reparenting by drag: a row's top quarter drops before it, the middle inside, the bottom quarter after (I12). The selection travels together and world transforms are kept.
- **Inspector:**
  - Header: one row with the entity's icon, its editable name and its id.
  - Components: Transform first. Each header has a fold, an icon, a spaced name and a ⋯ menu (Fold Others, Remove Component).
  - Fields: a 40/60 label split. The label scrubs numbers (8 px a step, Shift ×0.1, Alt ×10, I02); a click on the value types it, with units (I01, I05).
  - Field kinds: vectors with axis-coloured scrub tags, enum dropdowns, colour swatches with hex, checkboxes, and summaries for assets and lists.
  - Provenance: a 2 px bar marks it (authored, override, agent).
  - Add Component: searches the schema's components, with descriptions (I30).
- **Create:** a browser instead of a flat menu, after Unreal's Place Actors and Godot's Create Node.
  - Categories: Recent, All, then Basic, Shapes, Lights, Camera, Post-processing, Effects, Audio, Physics, Navigation, 2D, Gameplay.
  - Search: it takes typing at once and matches names, purposes and categories. Each row says what the object is for.
  - Footer: the components it adds, and "Inside <selection>".
  - Keys: arrows move, Enter creates. Entries show only when the schema has their components.
- **Assets:** Unity's Project window, with a folder tree beside breadcrumbs and a tile grid.
  - Tiles: they show rendered thumbnails, cached by content.
  - Opening: double-click opens a folder or a scene, and right click opens the Asset or Folder menu.
  - Placing: models and prefabs drag into the Scene and land on the ground under the pointer.
- **Console and Problems:** both read the diagnostics bus.
  - Console: severity chips with counts. Two-line rows give the message, then time, origin and source. It stays at the newest line while you're there (`follow_end`).
  - Problems: groups errors and warnings by file.
- **Agents:** presence (what each agent works on, View, Follow), then turns. Each turn shows its reason, who, when and what in counts, with Undo Turn; it expands into its ops.
- **History:** every transaction newest first, with Undo and Redo.
- **Palette (⌘K):** commands with shortcuts (⌘ ⇧ ⌥ on macOS) and the scene's entities.
- **Preferences (⌘,) and Project Settings:** one window generated from the settings table.
  - Layout: You/Project tabs, categories, and search over names and descriptions.
  - Controls: switches, segmented choices, dropdowns, number and text fields. A changed setting shows a dot and Reset.
- **Play cues:** a green line under the title bar while playing (amber while paused), and a labelled status chip (I40).
- **Problems:** each problem's typed fixes are one-click buttons, the same fixes agents apply.
- **Field menu:** Reset to Default, Copy Value, and Copy Path (`@scene#entity/Component.field`).

## stillkit additions this needed

Generic pieces went to stillkit, not Ara: native layers (#7), virtual lists (#8), pointer events (#6), styling (#5), file drops (#10, #11), ink cascade (#12), `flow_wrap` and `follow_end` (#15), key-up events (#16) and `focus_ring(false)` (#17). Ara pins stillkit's `ara` integration branch.

## Still to come

- **Windows Scene layer:** a Vulkan swapchain on the native HWND. Until then the studio is the default only on macOS.
- **Game view:** embedded players (D19). Until it lands, the studio's Play opens the player in its own window, and the "Play in the Game view" preference has no effect there. It needs shared-surface import, input capture and per-client stats, which today live in the legacy shell.
- **Pages:** the tool pages (material, VFX, animation, profiler, lighting, import), floating windows and drag-to-dock.
