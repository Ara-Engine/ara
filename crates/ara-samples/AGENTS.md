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
| `fetch [projects]` / `verify [projects]` | resumable, verified download into `<project>/assets/third_party/` (hard links from `samples/.cache/blobs`) / check only |
| `credits` | regenerate `samples/THIRD_PARTY_ASSETS.md` from the lock |
| `build outpost\|showcase` | author the project through `ara` (typed command batches via `ara do`), then `ara check` |
| `kits <project>` | write only the generated, git-ignored `assets/kit/` files (glTF + bin, suit palette); no document is authored. Release packaging runs it on the staged template copy (`--samples <stage>/templates`) |
| `capture <project> <look> [--quick] [--quality low\|medium\|high\|epic] [--reference spp]` | captures, per-pass timings and optional path-tracer errors → `target/ara-samples/captures/<project>/<look>/` |
| `play <project> <scene> <name> [--walk ticks]` | windowed `ara-player` through `ara mcp` (`ara_play`), presented + full captures |
| `editor <project> <scene> <name> [--camera x,y,z,yaw,pitch,distance]` | `ara-editor --screenshot`, framed over the Engine Protocol → `evidence/<name>-editor.png` |

`build`, `capture`, `play` and `editor` need release builds of `ara`,
`ara-import-worker`, `ara-player` and `ara-editor` (found next to this binary, in `target/release`, or via `ARA_BIN`,
`ARA_IMPORT_WORKER`). GPU commands need the Vulkan SDK on `PATH` and the
process-local layer isolation the renderer crates use (`VK_IMPLICIT_LAYER_PATH` =
an empty dir, `VK_LAYER_PATH` = the SDK `Bin`).

## Modules

- `lock`, `fetch`, `sources`, `credits`: the manifest, downloads (curl, `-C -`
  resume, BLAKE3 before publishing a blob), source resolution, attribution.
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
  `ARA_CAPTURE_SIGNALS=1` dumps direct/incident/radiance signals to scratch.
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

`cargo test -p ara-samples` (lock validation, kit geometry, source-pivot bounds, fold-in maths).
`ARA_AUDIT_GEOMETRY=1` also audits fetched source-model bounds in the Outpost layout test;
resolved world positions, ground-relative shots and bounds go under `target/ara-samples/`.
`cargo clippy -p ara-samples --all-targets -- -D warnings`.
