# ara-renderlab

Renderlab (`github.com/Ara-Engine/renderlab`) as an Ara renderer. Provisional: D35.
`Renderlab` implements `ara_runtime_render::ExternalRenderer`; a host installs it
with `Presenter::set_external_renderer`, and presents, captures and perf runs then
draw the extracted `RenderFrame` through Renderlab's `Engine`. The player's switch is
`--renderer ara|renderlab|renderlab-rt|renderlab-nonrt` (or `ARA_RENDERER`).
`ara-render` stays the default.

This is the only crate that names a Renderlab or wgpu type. Keep it that way: the
GPU layer under Renderlab is being measured against `ara-gpu`, and a swap must touch
this crate alone.

## API

- `Renderlab::new(&Options)`: creates Renderlab's engine on its own Vulkan device.
  Fails with `ARA_RENDERLAB_UNAVAILABLE` (no adapter, missing features, a Mac); hosts keep Ara's renderer.
- `PathChoice::{Auto, HardwareRt, NonRt}`: `Auto` prefers hardware RT. The non-RT
  path prints `ARA_RENDERLAB_NONRT_PATENT` (open patent question on its sun-shadow method).
- `Renderlab::path()`: the path in use.

## Layout

- `scene.rs`: draws to Renderlab meshes, materials and objects, shared by content
  key; GPU-skinned primitives posed from `RenderFrame::palettes`.
- `batch.rs`: past Renderlab's group limit, which static groups merge into world-space
  batches per material, and which (last resort) are left out.
- `texture.rs`: cooked KTX2 (BC7/BC5) to Renderlab's RGBA8 mip chains.
- `lighting.rs`: camera, sun, sky source, local lights, exposure and display settings.
- `environment.rs`: HDRI and gradient skies prepared for `Sky::Map` (CPU, once per sky, on a
  worker thread: 0.4 s for the gradient sky, 1.4 s for a 2k HDRI on twelve threads).
- `decals.rs`: `DecalView` to `Engine::set_decals`.
- `effects.rs`: `VisualEffect`s (Ara's VFX library) to Renderlab emitters, their sprite
  sheets from Ara's cooked atlas, and their lights.
- `terrain.rs`: resident terrain tiles and foliage instances as meshes.
- `bridge.rs`: the frame loop and the image handoff; `shared.rs`: the shared ring.
- `gaps.rs`: `ARA_RENDERLAB_UNSUPPORTED`/`ARA_RENDERLAB_LIMIT` reporting.

## Image handoff

Renderlab renders on its own device (D35: for weeks, until its passes move onto
`ara-gpu`). On Windows the frame crosses through a ring of images the two devices
share (`shared.rs`): Renderlab's device exports them exactly as `ara-gpu`'s
`ExportRing` does, wgpu copies the output into slot 0, and the presenter's device
copies it out with `ImportedSurface::copy_out_image`. Elsewhere, or when the two
devices are different adapters, the frame is read back and uploaded (the CPU copy,
3.3 to 4.1 ms at 1080p); the bridge prints once why. `ARA_RENDERLAB_HANDOFF=copy`
forces the copy. `PresentStats::external_handoff_ms` reports the cost either way.

## What the Renderlab path does not render yet

The switch is scaffolding (D35): the end state is one renderer, and the last column
is the plan for deleting `ara-render`. Each row reports `ARA_RENDERLAB_UNSUPPORTED`
once (`ARA_RENDERLAB_LIMIT` for the limit) when a frame uses it.

| Feature | On the Renderlab path today | What it would take | Blocks deleting `ara-render`? |
|---|---|---|---|
| Frame cost | On Showcase at 1080p (RTX 4070 SUPER, Renderlab `4652ea3`): 33 ms (RT) and 62 ms (non-RT) against 3.0 ms for `ara-render` at Epic; about 15 and 50 ms with `ARA_RENDERLAB_SKY=constant` | Renderlab's own work, not the bridge's. A bound sky map costs about 15 ms in its shading pass, in its reflection rays' environment lookups (reported to its thread; 38 ms in its mip-chain layout, which the bridge avoids). The rest is glossy reflections and its GI cache | Yes |
| Large levels on the non-RT path | Outpost fails: foliage is capped so instances and triangles fit the 32-bit ids of Renderlab's non-RT shadows (`ARA_RENDERLAB_LIMIT`), and then a GPU job outlives the driver's 10 s timer on the Radeon 760M (device lost; not yet run on the RTX). The RT path renders Outpost since the TLAS instance limit moved from 4,096 to the adapter's | 64-bit ids in Renderlab's virtual shadows; profile the long job on Outpost | Yes |
| More than 256 mesh-material groups | Past 256, opaque static groups that share a material merge into one world-space batch per material, fewest objects first (`batch.rs`); a batch whose members move is rebuilt each frame. Draws are left out only when merging still does not fit (more distinct materials than the limit). Terrain tiles take at most half the groups. Both report `ARA_RENDERLAB_LIMIT` | Renderlab#2 raised `TRACED_DRAWS` from 64 to 256 at no cost when unused on non-RT and 1 to 2% on RT (Radeon 760M). Beyond that: growable per-draw tables in Renderlab, or its virtual geometry | No: nothing is left out below 256 distinct materials |
| Terrain and foliage levels of detail, terrain layer maps, foliage wind | Resident tiles draw as full-resolution meshes (at most 128: Outpost leaves 16 of 144 out), coloured by their splat layers' base colour factors (white without layers); foliage instances draw their rule's model parts, standing still, at most 60,000 parts | A splat-blend program in WGSL for layer maps; Renderlab mesh LODs or its virtual geometry for distance; a vertex hook for wind | Yes |
| A sky light strength other than 1 | The sky lights the scene as brightly as it looks (HDRI and gradient skies are drawn and light the scene) | Separate light and background scales in Renderlab's `Sky::Map` | No |
| VFX graph effects | Library effects draw on Renderlab emitters (`effects.rs`): rates and first-tick bursts, point, sphere, box and cone spawns, per-axis velocity, gravity, drag, curl noise, one vortex and one attractor, a bounce plane, four-key colour and size curves, Ara's procedural sprite sheets played over each life (fire with its heat ramp, twice as tall), camera-, velocity- and ground-aligned sprites, and emitter lights as point lights (they light the scene too; Ara's light only particles). New effects start prewarmed to the presented tick; steps follow simulation ticks. Reported and left out: mesh-surface spawns, depth collisions, ribbons and mesh particles, events, material flipbook atlases, smoke lighting gradients, velocity shutter, more than 2,048 particles per emitter. Lit smoke reads paler and drifts less than Ara's (Renderlab's near-isotropic particle lighting) | Ribbons, mesh particles, depth collisions and events in Renderlab's emitters; a phase function for lit particles; multi-workgroup sort past 2,048 | Yes: ribbons, mesh particles and events |
| Decal emission and ambient occlusion, distance fade, renderers that refuse decals, decal maps above 512 texels | Base colour, roughness, metalness and normal are projected; the rest is left out, every opaque surface in the box receives, larger maps are reduced | Emission and occlusion channels and a per-object mask in Renderlab's decals; fade in the bridge | Yes: per-object opt-out |
| macOS and Metal | Ara's renderer draws (`ARA_RENDERLAB_UNAVAILABLE`) | A Metal backend under Renderlab, or Renderlab ported onto `ara-gpu` (D35) | Yes |
| Editor Scene view and multi-view | Not routed: one view per engine. The play process's presented image (windows, the embedded Game view, captures) works | More than one view per Renderlab `Engine` | Yes |
| Entity marks in captures, debug view modes, rendering debug switches, the path-traced reference | Captures have no legend; debug views and the reference fail with `ARA_VIEW_MODE_UNAVAILABLE`; switches are reported and ignored | An ID target and view modes in Renderlab's `Engine` output | Yes: agents and the editor pick through them |
| Automatic exposure | The bridge meters the displayed image toward middle grey | An `Exposure::Auto` in Renderlab with Ara's histogram metering | Yes |
| Tone mappers, grading, LUTs, depth of field | Renderlab's filmic curve, exposed so middle grey matches Ara's curves; bloom and motion blur follow the look | Map `PostSettings` fields; add the Neutral and AgX curves to Renderlab | Yes |
| Sky-atmosphere luminance factors and clouds, fog volumes, volumetric fog emission | Authored `SkyAtmosphere` and `HeightFog` map to Renderlab's medium and fog value for value (`atmosphere.rs`); the artistic factors, the cloud layer, fog volumes and fog emission are reported and left out | Luminance factors and clouds in Renderlab's atmosphere; local fog volumes need a renderer feature | Yes |
| Alpha masks on skinned meshes, transmission that varies over a surface | A masked material is cut into its geometry at load with Renderlab's `cutout` (the mask averaged to the cut grid's scale, so detail finer than a cell counts by coverage; a mask that hides nothing is not cut); cut meshes keep their normal maps on tangents derived from their uv mapping (`cutout.rs`). A blended material, by alpha factor or by its map's alpha, draws in Renderlab's transparent pass. Skinned masked primitives draw opaque; uniform transmission is glass | Cutting skinned primitives; per-texel transmission needs a renderer feature | Yes |
| Emissive maps on see-through materials or with their own texture coordinates | An opaque material's emissive map glows per texel: the map rides in its base-colour alpha and its metallic-roughness red and alpha (`texture::with_glow`), and global illumination receives the map's mean. Those two cases do not glow, reported. In an emissive surface's own reflections other emissive objects glow with its strength | An emissive texture slot in Renderlab's material; its own parameters at reflection hits | Yes |
| Material-graph materials, vertex paint, second UV set, AO maps | Base factors and maps on the standard program | Compile Ara material graphs to Renderlab WGSL programs | Yes |
| Rect, disc and tube lights; IES profiles | Wide spot or sphere of the same intensity; no profile | Area-light and profile support in Renderlab's light types | Yes |
| Texture compression and streaming | Textures decode to RGBA8 with full mip chains (about 5x the BC7 memory) | BC7/BC5 upload in Renderlab's texture path | Yes, for shipping memory |
| Meshes with degenerate tangents | Normal map dropped for that mesh (reported by name) | Regenerate tangents in the bridge (MikkTSpace) | No |
| Single-sided materials | Every material is double-sided | Renderlab's shadowed paths need double-sided materials today | No |
| Skinning with more than four influences | The four strongest, renormalized | Eight-influence skinning in Renderlab | No |
| 2D | Unchanged: `Presenter2d` keeps Ara's 2D renderer | Nothing for 3D work | No |

Reflection probes are not a gap: Renderlab traces reflections on both paths. Its
glossy band could leave bright speckles on glossy surfaces before `4652ea3` (a renderer
defect, since fixed there); `Options::max_reflection_roughness` (`ARA_RENDERLAB_REFLECTION_ROUGHNESS`
in the player) limits tracing, 0.05 keeping mirrors only.

Captures list Renderlab's own passes as `External.Pass.*` beside `External.Frame.Gpu`;
the rest of that span is its GI cache. Renderlab's profile of Showcase found no bridge
cost at `71ea355`: anisotropy, RGBA8 textures and the bridge's settings each moved the
frame by less than noise. `ARA_RENDERLAB_SKY=constant` (`Options::constant_sky`) lights the
scene with a constant sky of the sky image's mean radiance, to measure what the map costs.

## Test

- `cargo test -p ara-renderlab --lib`: pure conversions (no GPU).
- `cargo test -p ara-renderlab --test render`: a hand-built frame, an HDR sky and a
  601-group crowd (batching; boxes moved and removed) through the presenter on each
  path the adapter supports.
- `fixtures/gaps/`: `levels/crowd.scene` (600 boxes, 601 groups: batching) and
  `levels/effects.scene` (campfire, smoke, sparks, swirl). From that folder:
  `ARA_RENDERER=renderlab-rt ara capture --scene levels/effects.scene --ev100 4 --ticks 120`
  (crowd: `--ev100 10`); add `--video 60 --video-move 2,0,0 --video-yaw 20` for a moving
  camera, `--video 6 --video-yaw 300` for cuts.
- A GPU check renders a real scene: `ara-player --renderer renderlab-rt --headless
  --capture <png> --root <export>` (see the PR that added this crate for the commands).

## Gotchas

- Renderlab wants a full mip chain per texture and unit tangents orthogonal to the
  normals; a mesh it rejects with tangents is uploaded without them (`ARA_RENDERLAB_UNSUPPORTED`).
- Objects sharing a mesh and a material share one Renderlab draw group; a skinned
  primitive is its own group (its pose is per mesh).
- `Engine::capture` blocks for the frame. The path is synchronous: no frames in flight.
- The first frame compiles Renderlab's pipelines; its pipeline cache lives under the
  player's GPU cache in `renderlab/`. On NVIDIA that first start is slow (about 90 s
  for a simple scene at v0.1.3) and Renderlab saves the cache only when it exits
  cleanly. `ara capture` gives the player 120 s, so a first non-RT capture of Showcase
  fails and, the player being stopped, fails again. Reported to Renderlab.
- A Renderlab program gets its material's texels in `surface` only. At reflection hits
  on other objects (shaded with the reflecting surface's program) and at GI hits a
  lookup returns something else: the emissive programs read their map in `surface`,
  pass global illumination a plain parameter, and glow only on a material marked in
  its base-colour alpha (`GLOW_MARK`).
