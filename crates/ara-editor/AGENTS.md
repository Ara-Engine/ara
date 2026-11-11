# ara-editor

Native editor, in-process project authority and Engine Protocol server. The window is
the **studio** (`src/studio/`, stillkit, D34): read `src/studio/AGENTS.md` first. On Linux
its views need a Wayland session with subsurfaces (`studio::can_host`); elsewhere it opens
with no setup. `--headless` captures the Scene to `editor-scene.png` without a window
(`studio::headless`, used by release smoke tests and `ara hub open --headless`).

API:
- `Editor`: host, navigation commands, content batches, agent activity and per-turn undo.
- `state`: navigation/layout op document; deliberately separate from content undo.
- `world::EditWorldView`: the document extraction (replace with ara-sim later).
- `inspector`: schema fields, mixed values, units, provenance; compound fields edit per
  element (`path#pointer`).
- `gpu`: 64-byte instance ABI, raster/MSDF atlases, ordered draws and clip masks.
- `transport`: authenticated localhost EP and MCP; no project state on IO threads.
- `build.rs`: embeds `assets/brand/icons/engine/ara-engine.rc` (engine ICO).
- `open_project`: the layout-migration prompt before anything opens (its own small
  `ara_ui` window).

Every content change uses cmd.apply, including gizmo commits and diagnostic fixes.
Gizmo and typed-draft previews are transient renderer patches, never document writes.
Escape drops the preview; release commits one transform.set batch with its base.
`studio/scene_sync.rs` patches the Scene in place (`world/patch.rs`: moved meshes alone via
`update_mesh`, lights, `update_materials`, `rename`) for value edits, drafts and drags;
structure and mesh geometry changes, and new model publications, upload the whole scene.
`studio/viewport.rs` owns `SceneView`: retained colour/IDs without GPU waits, picking,
vertex/surface snapping, probes, Game preview cameras and captures.
View commands produce ara-ops transactions in a separate navigation document; the layout
persists canonically to `.ara/editor-layout.json`. `state::default_dock` is the default.

Play (D19): `play.*` through `PlayService` (registered in `Editor::open`); Stop also cancels
a starting Play. Multi-player Play is `editor.play` (`clients`, `dedicated`, simulated
`latencyMs`/`jitterMs`/`loss`). The Game view (`studio/game.rs`) shows each player's frames
through the play host's shared surfaces (`studio/player.rs`) and, outside Play, the
gameplay camera's preview (`studio/game_preview.rs`).
Keep "f…r" out of editor method names and summaries (the palette searches "fr").
One schema world: `Editor::open` uses `ara_play_host::open_project`, so the editor
validates and inspects exactly the engine components the player runs. The viewport
decodes `MeshRenderer` as `ara_runtime::components::MeshRenderer`; never add editor-only
component schemas. C# integration registers host services/schema types, not UI state.
`world/gltf.rs` retains one read-only publication reader and decoded models keyed
by asset ID/digest. `Editor::models` survives document edits. Unpublished models use
placeholders; failed reloads retain last good. Scene recovery uses three worker
attempts per generation and reports once.

Agents: `capture` jobs (`ara_capture`: the Scene with entity marks, the window, its layout)
are served by the studio's loop (`studio/capture.rs`, written by `capture::Work` on a
worker); probe jobs (`render.probe.*`) by `studio/probes.rs`.

Polish assets: Lucide SVGs (ISC), Inter, JetBrains Mono (OFL; THIRD_PARTY_NOTICES).
Raster IDs drive picking and hover through `world/channels.rs` (channel → entity; terrain is
one object). A raster Scene draws selection outlines into its frames (`Outlines::in_scene`);
overlays draw with the shown frame's camera and drag pose (`SceneView::shown_frame`).

Panel authoring: asset_tools owns isolated import jobs, cached off-thread thumbnails
(models, material spheres, VFX) and material node previews (`node_preview.rs`). authoring
registers graph/material/VFX/animation commands; graph is the shared culled canvas.
Material and VFX edits lower through their ASTs to graph ops; animation projections are
read-only and edit controller data. Host import sidecars save to `.import.json`.
Profiler views consume actual ara-profile frames/spans/GPU queue timings.

Commands: `catalog/` is what the studio offers (`table.rs` id, title, icon, keys,
summary; `menus.rs` the menus and context menus; `UNBUILT` commands show dimmed with
their reason). `keys.rs` holds chords and the Ara/Unreal/Unity/Blender presets;
`catalog::keymap` layers them with `input.shortcuts`, and `catalog::export` is
`editor.commands` (MCP). `extensions.rs` publishes C# commands/menus into it.
`settings.rs` holds the settings table; edits are ops on a private store (undoable,
`editor.settings.*`), user scope persists to `editor-settings.json` in
`settings::config_dir()` (`ARA_CONFIG_DIR`, else `%APPDATA%\Ara`) and project policy
to `settings/project.json`; marker name/main stay in `ara.project.json`.
Tests never touch the owner's profile: `.cargo/config.toml` gives every cargo-run
process `ARA_CONFIG_DIR`/`ARA_CRASH_DIR` under `target/`, `config_dir()` refuses the OS
profile under tests and cargo, and a headless run (`Options::isolate_profile`) starts
from a fresh `<output>/.ara-profile`. Never write user state anywhere else.
Scene quality: `EditorState.render_quality` mirrors `viewport.quality` (None = project);
commands `view.quality.{project,auto,low,medium,high,epic}`; agents: `editor.preferences`.
Scene convergence is three jitter cycles of completed frames, plus frames while auto
exposure adapts (at most 120 per input), then idle.
Scene camera: `state::Camera` (orbit pose, `fov_y_degrees` 10–120, default 50) feeds
`spec()`. `editor.camera` reads it or sets any subset (`entity` looks through a `Camera`).

D24: agent changes apply immediately; Agents is an activity feed with Undo turn.
Human gizmos, inspector scrubs and typing call host editor.gesture on begin/end.
`activity.rs`: agent turns glow 5 s, "agent is viewing" and overwrite (ARA2503) notices
with View/Follow; never move the human camera. `editor.scene.open {scene}` (path or ID;
agents move only their presence). Scenes open with their last camera (`view_camera.rs`),
else their gameplay camera, else framed content. Editor failures use ARA4500-4504
(`codes.rs`, catalog `editor.md`); `Editor::report_failure` re-logs an engine error's own
diagnostics.
Build (`build/`): exports the selected `settings/builds/*.export.json` profile after a
canonical save; ordinary Build creates a default profile when needed; `build.run` launches
the export's entry point.
Mesh Paint (`paint.rs` brush/fill/flood/pick math, `painter.rs` strokes, `editor.paint`):
presses on selected meshes paint per-instance colours previewed through
`Editor::paint_preview`; release commits one `vertexPaint.set` per primitive.

Gate with focused checks (`cargo clippy -p ara-editor --all-targets`, `cargo test -p
ara-editor`); GPU tests need the platform's GPU and, for material previews on macOS,
`ARA_METAL_SHADERCONVERTER`.
