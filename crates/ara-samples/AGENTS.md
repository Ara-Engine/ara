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
| `capture <project> <look> [--quick]` | fidelity captures + per-pass GPU timings → `target/ara-samples/captures/<project>/<look>/` |
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
- `kit`: generated glTF geometry (boxes/cylinders/spheres/ribbons with metre-based
  UVs over fetched texture sets) and derived glTFs of fetched models (`variant`:
  node selection, GLB → glTF, cut-out alpha merge, lamps switched on, image swaps).
- `build/`: `Ctx` (CLI batches, doc.create, project.setMain, import.initialize),
  `place` (props + separate collider entities, ground sampling, layout shift),
  `kits` (containers, hut, hangar shell, mast, fence, embers, panels…), `anim`
  (locomotion controllers via animation.create), `outpost`, `hangar`, `showcase`.
- `capture/`: `world` (play-host asset pipeline -> `Runtime`), `convert` (resolves
  lock/look assets, then calls ara-runtime-render's engine-owned fidelity adapter),
  `render` (shots, upscaled pass, strip, timings), `look` (the per-scene look file).
- `play`: MCP client over `ara mcp`.
- `editor`: switches the project's `main` scene for the run (the editor opens only
  `main`), then frames the view from a navigation-only `human` session, because an
  agent session's `editor.camera` moves only its presence marker.

Set `ARA_CAPTURE_OUTPUT_DIR` to opt into another capture destination. Ordinary
captures leave tracked sample evidence unchanged.

## Gotchas

- The fidelity view accepts at most 96 textures and 4096 meshes and has no
  instancing; the engine adapter currently merges by material, picks cooked LODs
  by world-space error and atlases. Dropped maps fold into factors.
- Looks hold what scenes cannot (shots, HDRI sky, ground material) and temporary
  tuning overrides (`components`, `sunLux`, `sunDirection`, `effects`). Sky, fog and
  post belong in the scene's `SkyAtmosphere`/`HeightFog`/`PostProcess`.
- `capture --quality low,medium,high,epic|all` captures each preset (Low through the
  baseline renderer); `--reference <spp>` scores presets against path-traced truth
  (linear radiance RMSE, sRGB RMSE, LDR FLIP). `ARA_CAPTURE_SHOTS=a,b` limits shots,
  `ARA_CAPTURE_SIGNALS=1` dumps direct/incident/radiance signals to scratch.
- Outpost layout code works in layout coordinates; `outpost::SHIFT` moves the
  layout into the middle of the 384 m terrain (the landscape renderer needs an
  origin-anchored terrain). Look shots are world coordinates.
- Keep generated files deterministic so a rebuild leaves `git status` clean apart
  from intended changes.
- Agent edits apply immediately under amended D24, with attribution, why and undo.
  Terrain strokes and foliage exclusions use stable IDs so rebuilds retain identity.

## Testing

`cargo test -p ara-samples` (lock validation, kit geometry, aim, fold-in maths).
`cargo clippy -p ara-samples --all-targets -- -D warnings`.
