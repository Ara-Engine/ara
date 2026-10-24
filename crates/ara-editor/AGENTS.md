# ara-editor

Native M2 editor shell, in-process project authority and Engine Protocol server.
The UI is ara-ui; its display list goes to the Vulkan adapter in `ara-editor-gpu`.
The viewport samples ara-render's offscreen image on that same device.

Public boundaries:
- `Editor`: host, navigation commands, content batches, agent activity and per-turn undo.
- `state`: navigation/layout op document; deliberately separate from content undo.
- `world::EditWorldView`: replace the document extraction with ara-sim later.
- `inspector`: schema-driven fields, mixed values, units and provenance.
- `shell`: retained panel tree and semantic interaction helpers.
- `gpu`: 64-byte instance ABI, raster/MSDF atlases, ordered draws and clip masks.
- `transport`: authenticated localhost EP and MCP; no project state on IO threads.
- `build.rs`: embeds `assets/brand/ara.ico` as the Windows exe icon (embed-resource;
  `rc.exe` ships with the Windows SDK MSVC already needs). The window icon comes from ara-platform.

Every content change uses cmd.apply, including gizmo commits and diagnostic fixes.
Gizmo and typed-draft previews are transient renderer patches, never document writes.
Escape drops the preview; release commits one transform.set batch with its base.
`app/scene_sync.rs` patches the Scene in place (`world/patch.rs`: moved meshes alone via
`update_mesh`, lights, `update_materials`, `rename`) for value edits, drafts and drags;
structure and mesh geometry changes, and new model publications, upload the whole scene.
View commands produce ara-ops transactions in a separate navigation document.
Persist layout with the canonical writer to `.ara/editor-layout.json`.

Play/Pause/Resume/Step/Stop bind `play.*` (ara-play-host's `PlayService`, registered
in `Editor::open`): Play opens the scene in a pooled `ara-player` window. Multi-player
Play (D19) is the palette's `editor.play` (`clients`, `dedicated`, per-client simulated
`latencyMs`/`jitterMs`/`loss`): one replicated window per player; `Editor::action` turns
its resolved params into `play.start`. Keep "f…r" out of editor method names and
summaries (the palette golden searches "fr"). The in-editor Game viewport through a
shared GPU texture (D19) is still to wire.
One schema world: `Editor::open` uses `ara_play_host::open_project`, so the editor
validates and inspects exactly the engine components the player runs. The viewport
decodes `MeshRenderer` as `ara_runtime::components::MeshRenderer` (same defaults,
materials and procedural shapes as Play); never add editor-only component schemas.
C# integration registers host services/schema types, not mutable UI state.
`world/gltf.rs` retains one read-only publication reader and decoded models keyed
by asset ID/digest. `Editor::models` survives document edits; extraction selects
the source node/primitive and pivot without reopening sidecars or decoding per
entity. Unpublished models use placeholders; failed reloads retain last good.

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
1440x900 at 1x and 2560x1440 at 2x, including palettes and the Agents feed.

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
`editor.settings.*`), user scope persists to `editor-settings.json` in
`settings::config_dir()` (`ARA_CONFIG_DIR`, else `%APPDATA%\Ara`) and project scope
to `ara.project.json`. `Editor::open` keeps user settings in memory.
Tests never touch the owner's profile: `.cargo/config.toml` gives every cargo-run
process `ARA_CONFIG_DIR`/`ARA_CRASH_DIR` under `target/`, `config_dir()` refuses the OS
profile under tests and cargo, and bounded runs (`Options::isolate_profile`) start
from a fresh `<output>/.ara-profile`. Never write user state anywhere else.
Scene quality: `EditorState.render_quality: Option<QualityChoice>` mirrors the user
preference `viewport.quality` (None = the project's `rendering.quality`, the
Project Settings > Rendering page); `state.quality` is derived in `apply_settings`
(false only for Low) and the window keeps `detected_quality` (GPU's Auto preset).
Commands `view.quality.{project,auto,low,medium,high,epic}` feed the Lit menu,
palette and `editor.commands`; agents may also use `editor.preferences.renderQuality`.
Floating windows: `DockLayout` floating roots map to owned OS windows
(`app/windows.rs`); each is a Shell `Surface` swapped into `self.ui` by
`enter/leave`, and renders through `NativeUi::select_target`. Rects persist in
the layout and off-screen windows are recovered at startup. Tab drags cross
windows through the global mouse; Esc cancels.
Paint-time decorations (swatches, row icons, graph, drop preview) go through
`Shell::paint_decorations`, which splices them under the popup layer; never append
draws after it. Icon tint classes must stay after `.icon` in editor.css.
Density: compact by default (`editor-comfortable.css` adds the other density),
22 px tree rows, 24 px property rows. CSS colors come from theme tokens; labels use
`color:inherit` and icons `tint` so state colors reach them.
`verify-ux.ps1` runs `--verify-ux`: menus, context menus, palette, settings,
float, tool tabs and empty states in both themes, plus `ux-report.json`
(viewport share, uncommanded buttons) and `commands.json`.

D24: agent changes apply immediately. No apply-policy setting, approval tickets
or pending agent queue. Agents is an activity feed with Undo turn (review.revert).
Human gizmos, inspector scrubs and typing call host editor.gesture on begin/end.
The host holds only overlapping fields for at most two seconds; other edits apply.

Phase 2 (alpha journey). `activity.rs`: agent turns glow 5 s, "agent is viewing"
and overwrite (ARA2503) notices with View/Follow; never move the human camera.
`editor.scene.open {scene}` (path or ID; agents move only their presence); scenes
keep cameras, first opens frame. Console/Problems (`shell/log_panels.rs`) read the
bus; editor failures use ARA4500-4504 (`codes.rs`, catalog `editor.md`) and
`Editor::report_failure` re-logs an engine error's own diagnostics. Prefabs:
`shell/prefab_ops.rs` (from selection, apply/revert overrides; root placement is
never an override). C#: `shell/scripts.rs` scaffolds a hot module + IDE .csproj.
Game view (D19): `app/game.rs` imports the play host's shared surfaces
(`PlayService::surfaces`, taken in `Editor::open`) and shows texture `game-view`;
`game_input` forwards keys/mouse to the selected client while captured (Esc
releases); a faulted world (`play.status`) shows its fault with Restart/Stop
instead of "Starting Play…". Play embeds unless `play.inGameView` is off. Build: `shell/build.rs`
runs ara-export with `presets/*.export.json` on a thread.
Bounded runs are offscreen (`Options::offscreen`: hidden windows, UI readback);
never add a capture route that shows a window. `--verify-journey` captures the
journey (`app/journey.rs`) and writes `journey.json` after every step.
`--verify-responsiveness` (`app/responsiveness.rs`) injects SDL-level clicks/moves into the
real loop and writes per-frame costs, UI work, click results and Play click-to-first-Game-
frame. Shell rebuilds reconcile into the retained `Ui` (`Shell::mount`), once per revision
(`rebuild_if_stale`); `Shell::resolved` caches the open scene. Only `scene_changed` state
(camera, view mode, hidden, quality) re-renders the Scene; icons re-rasterize on DPI only.
Tools: `shell/material_panel.rs` (preview + parameters, Graph|Source, compile
status; swatches are painted after layout, never raw colours in markup) and
`shell/vfx_panel.rs` (emitter stacks, preview-only Hide/Isolate, timeline driving
`asset_tools/vfx_stream.rs`: rate, restart, step, GPU counters updated in place).
Imports use the shared `import.initialize`/`import.configure` commands.
