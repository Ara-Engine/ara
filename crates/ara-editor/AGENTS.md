# ara-editor

Native M2 editor shell, in-process project authority and Engine Protocol server.
The UI is ara-ui; its display list goes to the Vulkan adapter in `ara-editor-gpu`.
The viewport samples ara-render's image on that device; initial renderer construction runs on a worker while loading pumps the authority and events.
Public boundaries:
- `Editor`: host, navigation commands, content batches, agent activity and per-turn undo.
- `state`: navigation/layout op document; deliberately separate from content undo.
- `world::EditWorldView`: replace the document extraction with ara-sim later.
- `inspector`: schema fields, mixed values, units, provenance; lists/maps/structs edit
  per element (`shell/compound.rs`, `path#pointer`). `shell`: retained panel tree.
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
View commands produce ara-ops transactions in a separate navigation document; the layout
persists canonically to `.ara/editor-layout.json`. `state::default_dock` is the default
(full-height sidebars; centre-only bottom dock); `float_size` pages open floating (`show_panel`).

Toolbar: idle Play + start-paused toggle; live Stop/Pause|Resume/Step, all `play.*`
(`PlayService`, registered in `Editor::open`); Stop also cancels a starting Play. Multi-player
Play (D19) is the palette's `editor.play` (`clients`, `dedicated`, per-client simulated
`latencyMs`/`jitterMs`/`loss`): one replicated window per player, via `play.start`.
Keep "f…r" out of editor method names and summaries (the palette golden searches "fr").
One schema world: `Editor::open` uses `ara_play_host::open_project`, so the editor
validates and inspects exactly the engine components the player runs. The viewport
decodes `MeshRenderer` as `ara_runtime::components::MeshRenderer` (same defaults,
materials and procedural shapes as Play); never add editor-only component schemas.
C# integration registers host services/schema types, not mutable UI state.
`world/gltf.rs` retains one read-only publication reader and decoded models keyed
by asset ID/digest. `Editor::models` survives document edits; extraction selects
the source node/primitive and pivot without reopening sidecars or decoding per
entity. Unpublished models use placeholders; failed reloads retain last good.

Gate with targeted check/test/clippy. The GPU gate (`run.ps1 -Mode Verify`, `tests/native.rs
--ignored`) needs validation layers, SDL3 and this machine's GPU. Goldens live in
`tests/goldens` (update only with `ARA_EDITOR_UPDATE_GOLDENS=1`); evidence in `evidence`.

Polish assets: Lucide SVGs (ISC), Inter, JetBrains Mono (OFL; THIRD_PARTY_NOTICES). `viewport.rs` owns SceneView and lazy GamePreview cameras: `preview_game` polls native content without drawing Scene; keep its handle while sampling, then release before the GPU.
Raster IDs drive picking, outlines and GPU-only hover through `world/channels.rs` (channel → entity; terrain is one object). Physical Scene extents and UI glyph
scale change together; keep the SDL window, editor logical, IME and UIA coordinate
conversions. Motion is paint-only, ≤140 ms, off with editor.preferences.
verify-polish.ps1 captures 1440x900 at 1x and 2560x1440 at 2x (palettes, Agents).

Panel authoring: asset_tools owns isolated import jobs and cached off-thread
thumbnails and material node previews (`node_preview.rs`). authoring registers graph/material/VFX/animation commands; graph is
the shared culled canvas and pre-shaped label cache. Preserve domain identities.
The shared play-host schema world owns engine kinds and material/VFX commands;
register only editor and generic graph commands on top of it.
Shared graph gestures return intents only. Material and VFX edits lower through their ASTs to
graph ops; animation projections are read-only and edit controller data. Animation canvas
annotations live in `.ara/editor/animation-<id>.graph`. Each graph page owns its canvas
(`Shell::graphs`, `<page>-canvas`); the focused one (`follow_focus`) carries `editor.canvas` and
takes graph commands. Labels are shaped once (scalable MSDF runs across pan/zoom). Keep
node/wire culling, the stable lookup table and the bounded thumbnail image pool.
Thumbnails: models via `asset_tools/studio.rs` (published materials, fitted 3/4 shot),
material spheres, VFX framed on simulated particles; 512 masters resampled to 2× each
view, on worker GPUs with off-thread IO, last good kept. Host import sidecars save to
`.import.json` while their asset IDs index source media; keep importer version policy.
Profiler views consume actual ara-profile frames/spans/GPU queue timings. Frozen
captures exclude open frames and write asynchronously to `.ara/cache/profiles`.
`--verify-panels` on a scratch project runs real import/reimport jobs, an asset
drag and graph create/connect/undo, then captures every panel in both themes and
measures 1,000 warm input-to-present samples with 500 nodes open.

UX v2 (D28). `shell/commands` is the one command registry: `table.rs` specs (id,
title, icon, keys, availability, label, run), `menus.rs` the six menus and every
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
Scene quality: `EditorState.render_quality` mirrors `viewport.quality` (None = project);
`state.quality` derives in `apply_settings` (false only for Low); the window keeps Auto's `detected_quality`. Commands
`view.quality.{project,auto,low,medium,high,epic}`; agents: `editor.preferences`.
`SceneView` retains completed color/IDs without GPU waits; input coalesces while busy. Scene convergence is three jitter cycles of completed frames, then idle; preview work excludes that budget. Startup warms one camera quality; panel targets allocate on the worker. Capture/shutdown drain explicitly (`ara-render/AGENTS.md`).
Scene camera: `state::Camera` (orbit pose, `fov_y_degrees` 10–120, default 50) feeds
`spec()`. `editor.camera` reads it (no fields) or sets any subset (`fovYDegrees`;
`entity` looks through a scene `Camera`). UI: toolbar FOV field, Camera menu presets, `view.camera.align`.
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
`verify-ux.ps1` runs `--verify-ux`: menus, context menus, palette, settings, float, tool tabs and empty states in both themes,
plus `ux-report.json` (viewport share, uncommanded buttons), `commands.json` and `drags.json` (`app/drags.rs`: SDL drags of
every splitter, tab tear-off/dock-into/reattach at UI scale 1 and 1.25 via `Shell::desktop_pointer`; no effect fails the route).

D24: agent changes apply immediately. No apply-policy setting, approval tickets
or pending agent queue. Agents is an activity feed with Undo turn (review.revert).
Human gizmos, inspector scrubs and typing call host editor.gesture on begin/end.
The host holds only overlapping fields for at most two seconds; other edits apply.

Phase 2 (alpha journey). `activity.rs`: agent turns glow 5 s, "agent is viewing"
and overwrite (ARA2503) notices with View/Follow; never move the human camera.
`editor.scene.open {scene}` (path or ID; agents move only their presence). Scenes open
with their last camera (`view_camera.rs`, a user preference), else their gameplay
camera, else framed content; F/Frame All skip terrain and ground unless selected. Console/Problems (`shell/log_panels.rs`) read the
bus; editor failures use ARA4500-4504 (`codes.rs`, catalog `editor.md`) and
`Editor::report_failure` re-logs an engine error's own diagnostics. Prefabs:
`shell/prefab_ops.rs` (from selection, apply/revert overrides; root placement is
never an override). C#: `shell/scripts.rs` scaffolds a hot module + IDE .csproj.
Game view (D19): `app/game.rs` imports the play host's shared surfaces
(`PlayService::surfaces`, taken in `Editor::open`) and shows texture `game-view`;
`game_input` forwards keys/mouse to the selected client while captured (Esc releases).
Play starts in the background; `Shell::read_play_status` shows its phase, a failed start
or a fault. Embeds unless `play.inGameView` is off. Outside Play `app/game_preview.rs`
renders `Shell::gameplay_camera` (`SceneView::preview_game`) as `game-preview`.
Build (`shell/build.rs`): `build.current` exports the main scene with the first
`presets/*.export.json` (else `build.profile.create` writes the portable, unpinned
Windows runtime profile) into `builds/<profile>-<time>/build` on a thread, pinning
this engine's runtime template (player + PDB, cooked shaders) under `.ara/export/`;
C# modules ship NativeAOT. `build.run` launches it (offscreen: headless, `run.png`). Bounded runs are offscreen (hidden
windows, UI readback); no capture route shows a window. `--verify-journey` (`app/journey.rs`) writes `journey.json` after
every step; `--verify-first-hour` (`app/qa.rs`) replays a new user's first hour on a fresh (Hub-created) project into
`first-hour.json`. `--verify-responsiveness` (`app/responsiveness.rs`, read by `ara-dev d26`) injects SDL-level input into
the real loop: per-frame costs, clicks, typing, drags, Play, startup. Shell rebuilds reconcile into the retained `Ui` once
per revision (`rebuild_if_stale`), menubar, status bar, area headers and tab bodies as parts (`Ui::reconcile_parts`);
inactive `panels::keep_alive` tabs stay mounted, hidden (geometry via `Shell::shown_node`). Memoized panels
(`shell/memo.rs`: Assets) reuse markup+bindings while their input digest holds; `cargo test` sets `ARA_EDITOR_MEMO_CHECK=1`,
which regenerates each reuse and fails on a difference. A commit frame resolves once (`Shell::document_view`; the Shell and
`editor.select`/`frame` use `world::SceneResolver`, which re-resolves only the records the new transactions touched), rebuilds
once (`coalesce_rebuilds`), keeps Hierarchy rows unless the ops change what rows show, and defers the Scene a frame.
`ImportProgress` drives `loading/` (the window before the main one, `--verify-loading`) and
`shell/imports.rs` (chip, Assets badges, Scene notice; `tests/loading.rs`); names drop file extensions (`shell/names.rs`). Only `scene_changed` state re-renders the Scene.
Tool pages (`docs/design/editor-tool-pages.md`) share a breadcrumb toolbar, side column,
status strip; `graph/style.rs` names nodes/pins; swatches paint after layout. VFX Hide/Isolate
are preview-only; its timeline drives `vfx_stream`. In Play the Animation page polls
`play.read <entity>.AnimatorState` (`AnimationLive`). Imports use `import.initialize`/`configure`.
