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
Importer sidecars and material instances currently both claim i_; register graph
material commands without the conflicting material-instance schema in this host.

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

