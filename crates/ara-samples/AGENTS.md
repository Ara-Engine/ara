# ara-samples

The real-art sample projects under `samples/` (`outpost`: Outpost at dusk + the night
hangar; `showcase`: material and character showcase) and the tool that fetches,
builds, captures and plays them. Third-party binaries never enter git: the lock
pins them, `fetch` restores them.

## Commands

`cargo run --release -p ara-samples -- <command>` (release: captures time the GPU).

| Command | Does |
|---|---|
| `add-polyhaven <id> --kind model\|texture\|hdri --resolution 1k --projects a,b` | resolve, download, BLAKE3 and pin a Poly Haven asset (models also pin their opacity maps) |
| `add-urls <id> --title --source --license --authors --projects path=url..` | pin explicit files with explicit attribution |
| `fetch [projects]` / `verify [projects]` | resumable, verified download into `<project>/content/third_party/` (hard links from `samples/.cache/blobs`) / check only |
| `credits` | regenerate `samples/THIRD_PARTY_ASSETS.md` from the lock |
| `build outpost\|showcase` | author the project through `ara` (typed command batches via `ara do`), then `ara check` |
| `game` | publish Outpost C# types through headless Play, then author its salvage loop, character, caches, doors, drones, animation and audio through typed commands |
| `kits <project>` | write only the generated, git-ignored `content/kit/` files (glTF + bin, suit palette); no document is authored. Release packaging runs it on the staged template copy (`--samples <stage>/templates`) |
| `capture <project> <look> [--quick] [--quality low\|medium\|high\|epic] [--reference spp]` | captures, per-pass timings and optional path-tracer errors → `target/ara-samples/captures/<project>/<look>/` |
| `play <project> <scene> <name> [--walk ticks]` | windowed `ara-player` through `ara mcp` (`ara_play`), presented + full captures |
| `editor <project> <scene> <name> [--camera x,y,z,yaw,pitch,distance]` | `ara-editor --screenshot`, framed over the Engine Protocol → `target/ara-samples/evidence/<project>/<name>-editor.png` |

`build`, `capture`, `play` and `editor` need release builds of `ara`,
`ara-import-worker`, `ara-player` and `ara-editor` (found next to this binary, in `target/release`, or via `ARA_BIN`,
`ARA_IMPORT_WORKER`). GPU commands need the Vulkan SDK on `PATH` and the
process-local layer isolation the renderer crates use (`VK_IMPLICIT_LAYER_PATH` =
an empty dir, `VK_LAYER_PATH` = the SDK `Bin`).

## Modules

- `lock`, `fetch`, `sources`, `credits`: the manifest, downloads (curl, `-C -`
  resume, BLAKE3 and size before publishing a blob), source resolution, attribution.
  File-level `mirrors` are optional and ordered: try `url`, then each mirror,
  against the same pins. Keep partial transfers separate per URL and retain every
  attempt's curl/HTTP or integrity error on total failure. Use `Lock::write` for
  lock edits; keep source pages, authors and licenses unchanged when adding mirrors.
  Release packaging copies this lock into Hub templates. Maintainers publish the
  pinned Kenney ZIPs to `sample-assets-v1`; agents do not upload release assets.
- `kit`: generated glTF geometry over fetched texture sets, and the graphite suit
  palette image. Original models stay immutable; the importer merges opacity maps.
- `build/`: `Ctx` (CLI batches, document lifecycle, project main, shared imports),
  `place` (props with centered colliders and ground sampling), `native` (source-node
  prefabs and lamp emission), `bounds` (source-pivot bounds), `materials` (native
  surface graphs), `kits` (containers, hut, hangar shell, mast, embers, panels),
  `anim` (locomotion controllers), `outpost`, `hangar`, `showcase`.
- `capture/`: `world` (play-host asset pipeline → `Runtime`), `convert` (resolves
  lock/look assets, then calls ara-runtime-render's engine-owned fidelity adapter),
  `render` (shots, upscaled pass, strip, timings), `look` (the per-scene look file).
- `play`: MCP client over `ara mcp`.
- `game/`: repeatable co-op game authoring; `tests/samples/outpost/coop.ps1` drives
  four real bot players, late join, private inventories, captures and authority replay.
- `editor`: switches the project's `main` scene for the run (the editor opens only
  `main`), then frames the view from a navigation-only `human` session, because an
  agent session's `editor.camera` moves only its presence marker.

Set `ARA_CAPTURE_OUTPUT_DIR` to opt into another capture destination. Ordinary
captures leave tracked sample evidence unchanged.

## Gotchas

- Fidelity and path-tracer captures use the shipping native adapter: shared local
  geometry, prefab foliage, material overrides and compressed texture residency.
  No texture atlas or world-vertex baking. Low uses the actual raster presenter.
- Looks hold what scenes cannot (shots, HDRI sky, ground material) and temporary
  tuning overrides (`components`, `sunLux`, `sunDirection`, `effects`). Sky, fog and
  post belong in the scene's `SkyAtmosphere`/`HeightFog`/`PostProcess`.
- `capture --quality low,medium,high,epic|all` captures each preset (Low through the
  baseline renderer); `--reference <spp>` scores presets against path-traced truth
  (linear radiance RMSE, sRGB RMSE, LDR FLIP). `ARA_CAPTURE_SHOTS=a,b` limits shots,
  `ARA_CAPTURE_SIGNALS=1` dumps direct/incident/radiance signals to scratch,
  `ARA_CAPTURE_SCORED=1` saves each raster frame FLIP scored (`*-scored.png`, at the
  reference's exposure). `ARA_CAPTURE_LIVE=N` saves every 4th of N live raster
  frames as Play shows them (no capture reset), walking (`ARA_CAPTURE_MOTION`) or
  dollying (`ARA_CAPTURE_DOLLY=<m/s>`); `ARA_CAPTURE_VIEW=albedo|normals|cascades|material`.
- `ARA_CAPTURE_AUTHORED_ONLY=1` keeps look cameras/ticks but disables temporary
  environment, material and effect overrides for matched four-path evidence.
- Outpost's exterior uses one world space in metres: terrain and resident chunks span `[-96, 288]` on X/Z around the compound at `(96, 96)`, and props, roads, foliage exclusions, world-space nav bounds and look poses use those same coordinates with parent/prefab transforms applied once.
  Terrain graph UVs are world XZ metres, scaled by `1 / texture.meters`.
- Rebuild cleanup uses attributed `doc.delete` transactions and retains `.ara/journal`.
  Only generated non-document assets and scratch directories are removed directly.
- Keep generated files deterministic so a rebuild leaves `git status` clean apart
  from intended changes.
- Agent edits apply immediately under amended D24, with attribution, why and undo.
  Terrain strokes and foliage exclusions use stable IDs so rebuilds retain identity.
  Outpost's 144 `TerrainChunk` entities are children of `e_terrain` (identity transform,
  world-space heights): one Terrain row in the Hierarchy, which starts large groups collapsed.

## Testing

`ara-samples game` also authors Outpost's native `content/levels/menu.scene` through entity
and prefab commands; it shares the compound art but contains no gameplay components.

`cargo test -p ara-samples` (lock validation, mirror fallback/integrity/errors,
kit geometry, source-pivot bounds, fold-in maths). Fetch tests use curl `file://`
URLs and scratch files under `ARA_TEST_ROOT` or `target/`, without network access.
`ARA_AUDIT_GEOMETRY=1` also audits fetched source-model bounds in the Outpost layout test;
resolved world positions, ground-relative shots and bounds go under `target/ara-samples/`.
`cargo clippy -p ara-samples --all-targets -- -D warnings`.
`cargo test -p ara-importers --test sample_versions` catches sidecar version drift;
ARA_UPDATE_DEMO=1 opts into regeneration from the installed importer registry.
