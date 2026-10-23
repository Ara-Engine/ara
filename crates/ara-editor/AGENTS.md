# ara-editor

Native M2 editor shell, in-process project authority and Engine Protocol server.
The UI is ara-ui; its display list goes to the Vulkan adapter in `ara-editor-gpu`.
The viewport samples ara-render's offscreen image on that same device.

Public boundaries:
- `Editor`: host, navigation commands, content batches, review, deferred agent policy.
- `state`: navigation/layout op document; deliberately separate from content undo.
- `world::EditWorldView`: replace the document extraction with ara-sim later.
- `inspector`: schema-driven fields, mixed values, units and provenance.
- `shell`: retained panel tree and semantic interaction helpers.
- `gpu`: 64-byte instance ABI, raster/MSDF atlases, ordered draws and clip masks.
- `transport`: authenticated localhost EP and MCP; no project state on IO threads.

Every content change uses cmd.apply, including gizmo commits and diagnostic fixes.
Gizmo preview is transient extraction data, never a side-channel document write.
Escape drops the preview; release commits one transform.set batch with its base.
View commands produce ara-ops transactions in a separate navigation document.
Persist layout with the canonical writer to `.ara/editor-layout.json`.

Play/Pause/Resume/Step/Stop bind `play.*` (ara-play-host's `PlayService`, registered
in `Editor::open`): Play opens the scene in a pooled `ara-player` window. The in-editor
Game viewport through a shared GPU texture (D19) is still to wire.
One schema world: `Editor::open` uses `ara_play_host::open_project`, so the editor
validates and inspects exactly the engine components the player runs. The viewport
decodes `MeshRenderer` as `ara_runtime::components::MeshRenderer` (same defaults,
materials and procedural shapes as Play); never add editor-only component schemas.
C# integration registers host services/schema types, not mutable UI state.
The temporary glTF loader is isolated in `world/gltf.rs`; replace with ara-asset.

Set CARGO_BUILD_JOBS=2. Use `run.ps1 -Mode Check` for the local gates or targeted
cargo check/test/clippy commands. The GPU gate uses process-local Vulkan loader
isolation and requires validation layers, SDL3 and this machine's GPU.
`run.ps1 -Mode Verify` runs the actual window, protocol interaction scenario,
GPU/CPU comparison, captures both themes and records raw timing samples.
Intentional goldens live in `tests/goldens`; evidence lives in `evidence`.
Never update goldens implicitly during ordinary tests.

Polish assets: bundled Lucide SVGs (ISC), Inter and JetBrains Mono (OFL); see
THIRD_PARTY_NOTICES.md. Icons rasterize only on theme/DPI changes. The fidelity
view and its bounded settling live in app/viewport.rs; destroy it before its GPU.
Raster IDs drive picking, silhouette outlines and GPU-only hover. Physical Scene
extents and UI glyph raster scale change together. Keep the separate SDL window,
editor logical, IME and physical UIA coordinate conversions. Motion is paint-only,
bounded to 140 ms and disabled by editor.preferences. verify-polish.ps1 captures
1440x900 at 1x and 2560x1440 at 2x, including palettes and review cards.

Panel authoring: asset_tools owns isolated import jobs and cached off-thread
thumbnails. authoring registers graph/material/VFX/animation commands; graph is
the shared culled canvas and pre-shaped label cache. Preserve domain identities.
The shared play-host schema world owns engine kinds and material/VFX commands;
register only editor and generic graph commands on top of it.

Shared graph gestures return intents only. Material and VFX edits lower through
their ASTs to graph ops; animation projections are read-only and edit controller
data. Animation canvas annotations live in `.ara/editor/animation-<id>.graph`.
Graph labels are shaped once and use scalable MSDF runs across pan/zoom. Keep
node/wire culling, the stable lookup table and the bounded thumbnail image pool.
Generated material previews and fixed-tick VFX previews own worker GPU devices;
all thumbnail decode/cook/cache IO is off the UI thread. Failures keep last-good
images and display diagnostics. Host import sidecars save to `.import.json`
while their asset IDs index source media; do not alter importer version policy.
Profiler views consume actual ara-profile frames/spans/GPU queue timings. Frozen
captures exclude open frames and write asynchronously to `.ara/cache/profiles`.
`--verify-panels` on a scratch project runs real import/reimport jobs, an asset
drag and graph create/connect/undo, then captures every panel in both themes and
measures 1,000 warm input-to-present samples with 500 nodes open.


UX v2 (D23). `shell/commands` is the one command registry: `table.rs` specs (id,
title, icon, keys, availability, label, run), `menus.rs` the nine menus and every
context menu, `keys.rs` chords and the Ara/Unreal/Unity/Blender presets. Menus,
palette, context menus, shortcuts, toolbar buttons and `editor.commands` (MCP) all
read it; a disabled command returns `Avail::Blocked(reason)` and a deferred one is
`Run::Later(reason)`. New buttons bind `Binding::Command`; `tests/ux.rs` rejects
uncommanded buttons. Context-menu sources are `Shell::context_at`.
`settings.rs` holds the settings table; edits are ops on a private store (undoable,
`editor.settings.*`), user scope persists to `%APPDATA%\Ara\editor-settings.json`
and project scope to `ara.project.json`. `Editor::open` keeps user settings in
memory; the app passes the real path via `open_with`.
Floating windows: `DockLayout` floating roots map to owned OS windows
(`app/windows.rs`); each is a Shell `Surface` swapped into `self.ui` by
`enter/leave`, and renders through `NativeUi::select_target`. Rects persist in
the layout and off-screen windows are recovered at startup. Tab drags cross
windows through the global mouse; Esc cancels.
Density: compact by default (`editor-comfortable.css` adds the other density),
22 px tree rows, 24 px property rows. CSS colors come from theme tokens; labels use
`color:inherit` and icons `tint` so state colors reach them.
`verify-ux.ps1` runs `--verify-ux`: menus, context menus, palette, settings,
float, tool tabs and empty states in both themes, plus `ux-report.json`
(viewport share, uncommanded buttons) and `commands.json`.
