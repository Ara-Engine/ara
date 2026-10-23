# Ara 2D toolset integration

The four new crates are discovered by the root workspace wildcard; no root manifest
change is needed. The integration deliberately keeps protocol/editor routing in the
host's ownership. Register `ara_2d::register`, `ara_physics2d::register` and both
`commands::register` functions in the shared SchemaDb/Registry. Install their
`register_sim` bridges before constructing EditWorld or Session. Diagnostics use
ARA3601 (2D authoring) and ARA3602 (physics authoring).

Install `ara_import2d::{AtlasImporter, TilemapImporter}` in an ara-asset worker
registry, never on the editor frame. Atlas settings crop source PNGs and pack them
with deterministic gutters/pivots/borders; optional dependency PNG paths are supplied
as immutable ImportInput snapshots by the host. Canonical metadata has no GPU slots.
AR2A v1 is an LE versioned header followed by RGBA8; color uploads are SRGB and
normal uploads UNORM. A tilemap's canonical text contains its complete source cells;
AR2C 32x32 tile/terrain payloads are disposable content-addressed cache objects.
`AtlasImage::from_cooked` validates pixels before upload. Source import settings and
assets can use ara-asset's existing typed sidecar mutation API and D20 publication.

Submit all pointer samples between pointer-down/up in one ara-cmd Batch, or provide
all samples to `tilemap.paint`. No document changes occur before that transaction.
`tilemap.fill` is a bounded connected flood fill, `tilemap.erase` uses the same cell
ops, and `tilemap.rule.apply` reads one immutable neighborhood snapshot. Stable layer
IDs use `l_ground` syntax. Each changed cell has its own op/precondition, so disjoint
edits retain ordinary kernel collaboration and per-author undo behavior.

`Scene2d::extract` consumes a fixed-tick ECS snapshot and boundary-published assets.
It evaluates flipbooks at the supplied tick, composes XY hierarchy transforms and
uses Body2d's absolute pose for physics entities. `Renderer2d::new` uploads that
immutable extraction; stage replacement renderers off the human's frame and swap
only at allowed frame boundaries, respecting the host's last-good/apply policy.
Ordinary camera frames update constants only. World/material extraction changes
require another staged extraction; GPU instance streaming is not implicit.

Camera2d uses art pixels/metre, integer zoom and nearest sampling in pixel mode.
Renderer batches preserve sorting layer/order and stable entity identity. Nine-slice
draws use 54 procedural vertices; ordinary sprites use six. Tile layer chunks retain
separate draw/culling bounds. Normal lights and segment shadows are opt-in per sprite
or tile definition. Unlit draws bypass light/shadow loops. Main-renderer exposure,
ACES fit and sRGB output are reused; pixel art has no jitter/TAA. The current common
Vulkan heap bounds the resident atlas count; overflow is a structured GPU error.

`capture` uses the shared PNG, entity IDs, picking, marks, comparison and provenance
contract. Route 2D protocol captures here when a view selects Camera2d; this crate
does not change the daemon's router. Lit/albedo/normals/entity_ids/shadow visibility
views are implemented. The orthographic CameraSpec and external capture-channel
constructor are additive shared hooks, as are ToneMap and DrawProcedural.

On Windows, set SDL3_LIB_DIR to the existing main renderer SDL SDK when it is not
installed in ara-platform/native. `run.ps1` scopes Vulkan's implicit-layer workaround
and CARGO_BUILD_JOBS=2. Run `-Mode Cook` only after shader changes, `-Mode Capture` for
the sample, `-Mode Benchmark` for 100k sprites, and `-Mode Check` for local gates.
The sample's source PNG/settings/scene are committed. Import workers rebuild cache
objects on a miss; no derived atlas/chunk payloads are committed as source assets.
The `platformer prepare` example deliberately regenerates only those sample sources.

GPU measurements are actual Vulkan timestamps, 8 warm-up + 120 measured frames at
1280x720, with immutable resident instances, unlit sprites and one frame in flight.
Sprite, post and diagnostic readback costs are reported separately; they do not
measure dynamic CPU extraction or import. The before-hierarchy baseline and final
measurement are in `measurements/`. Reference images/provenance are in tests/goldens.
