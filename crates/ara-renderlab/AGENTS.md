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
- `texture.rs`: cooked KTX2 (BC7/BC5) to Renderlab's RGBA8 mip chains.
- `lighting.rs`: camera, sun, sky, local lights, exposure and display settings.
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
| Frame cost | On Showcase at 1080p (RTX 4070 SUPER, release build, Renderlab `71ea355`): 11.0 ms (RT) and 43.1 ms (non-RT) against 3.0 ms for `ara-render` at Epic | Renderlab's own work, not the bridge's. RT: glossy reflections (about 5 ms) and its GI cache (about 5 ms). Non-RT: shading with glossy reflections (about 22 ms) and its GI cache (about 17 ms) | Yes |
| More than 64 mesh-material groups | Draws past the 64th group are left out (`ARA_RENDERLAB_LIMIT`) | Raise Renderlab's `TRACED_DRAWS` (a renderer change), or merge static draws per material in the bridge | Yes: real levels exceed it |
| Terrain and foliage | Not drawn | Feed landscape tiles and foliage instances as meshes; foliage cards through Renderlab's `cutout` | Yes |
| Environment-map (HDRI) and gradient skies | A constant sky of the map's mean radiance | Prefilter the map at load into Renderlab's `PreparedEnvironment` (its cook does this offline) and use `Sky::Map` | Yes |
| VFX graph effects | Not drawn | Map `EffectView` emitters onto Renderlab's GPU emitters | Yes |
| Decals | Not projected | Map `DecalView` to `Engine::set_decals` | Yes |
| macOS and Metal | Ara's renderer draws (`ARA_RENDERLAB_UNAVAILABLE`) | A Metal backend under Renderlab, or Renderlab ported onto `ara-gpu` (D35) | Yes |
| Editor Scene view and multi-view | Not routed: one view per engine. The play process's presented image (windows, the embedded Game view, captures) works | More than one view per Renderlab `Engine` | Yes |
| Entity marks in captures, debug view modes, rendering debug switches, the path-traced reference | Captures have no legend; debug views and the reference fail with `ARA_VIEW_MODE_UNAVAILABLE`; switches are reported and ignored | An ID target and view modes in Renderlab's `Engine` output | Yes: agents and the editor pick through them |
| Automatic exposure | The bridge meters the displayed image toward middle grey | An `Exposure::Auto` in Renderlab with Ara's histogram metering | Yes |
| Tone mappers, grading, LUTs, depth of field | Renderlab's filmic curve; bloom and motion blur follow the look | Map `PostSettings` fields; add the Neutral and AgX curves to Renderlab | Yes |
| Authored sky-atmosphere and fog parameters, fog volumes | Renderlab's default Earth atmosphere and fog medium; volumes not drawn | Map `SkyAtmosphere`/`HeightFog` to `AtmosphereMedium`/`HeightFog`; fog volumes need a renderer feature | Yes |
| Alpha masks, opacity maps, transmission that varies over a surface | Drawn opaque (uniform alpha and uniform transmission are glass) | Cut masks into geometry at load (`cutout`); per-texel transmission needs a renderer feature | Yes |
| Emissive maps | No emission (emissive factors glow and light the scene through GI) | Sample an emissive map in the bridge's emissive program | Yes |
| Material-graph materials, vertex paint, second UV set, AO maps | Base factors and maps on the standard program | Compile Ara material graphs to Renderlab WGSL programs | Yes |
| Rect, disc and tube lights; IES profiles | Wide spot or sphere of the same intensity; no profile | Area-light and profile support in Renderlab's light types | Yes |
| Texture compression and streaming | Textures decode to RGBA8 with full mip chains (about 5x the BC7 memory) | BC7/BC5 upload in Renderlab's texture path | Yes, for shipping memory |
| Meshes with degenerate tangents | Normal map dropped for that mesh (reported by name) | Regenerate tangents in the bridge (MikkTSpace) | No |
| Single-sided materials | Every material is double-sided | Renderlab's shadowed paths need double-sided materials today | No |
| Skinning with more than four influences | The four strongest, renormalized | Eight-influence skinning in Renderlab | No |
| 2D | Unchanged: `Presenter2d` keeps Ara's 2D renderer | Nothing for 3D work | No |

Reflection probes are not a gap: Renderlab traces reflections on both paths. Its
glossy band can leave bright speckles on glossy surfaces at `71ea355` (a renderer
defect); `Options::max_reflection_roughness` (`ARA_RENDERLAB_REFLECTION_ROUGHNESS`
in the player) limits tracing, 0.05 keeping mirrors only.

Captures list Renderlab's own passes as `External.Pass.*` beside `External.Frame.Gpu`;
the rest of that span is its GI cache. Renderlab's profile of Showcase found no bridge
cost: anisotropy, the constant sky, RGBA8 textures and the bridge's settings each move
the frame by less than noise.

## Test

- `cargo test -p ara-renderlab`: pure conversions (no GPU).
- A GPU check renders a real scene: `ara-player --renderer renderlab-rt --headless
  --capture <png> --root <export>` (see the PR that added this crate for the commands).

## Gotchas

- Renderlab wants a full mip chain per texture and unit tangents orthogonal to the
  normals; a mesh it rejects with tangents is uploaded without them (`ARA_RENDERLAB_UNSUPPORTED`).
- Objects sharing a mesh and a material share one Renderlab draw group; a skinned
  primitive is its own group (its pose is per mesh).
- `Engine::capture` blocks for the frame. The path is synchronous: no frames in flight.
- The first frame compiles Renderlab's pipelines; its pipeline cache lives under the
  player's GPU cache in `renderlab/`.
