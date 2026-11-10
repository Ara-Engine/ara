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
- `terrain.rs`: resident terrain tiles and foliage instances as meshes; `splat.rs`:
  the tiles' layer maps (atlas, weights and the splat program).
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

The switch is scaffolding (D35): the end state is one renderer, and the table is the
plan for deleting `ara-render`. The last column names the thread working each row:
take an unassigned row, or ask its owner, so two threads never map the same thing. Each row reports `ARA_RENDERLAB_UNSUPPORTED`
once (`ARA_RENDERLAB_LIMIT` for the limit) when a frame uses it.

| Feature | On the Renderlab path today | What it would take | Blocks deleting `ara-render`? | Owner |
|---|---|---|---|---|
| Frame cost | On Showcase at 1080p (RTX 4070 SUPER, Renderlab `4652ea3`): 33 ms (RT) and 62 ms (non-RT) against 3.0 ms for `ara-render` at Epic; about 15 and 50 ms with `ARA_RENDERLAB_SKY=constant`. Radeon 760M, Renderlab `f820af9`, RT: 150.0 ms with a constant sky, 172.7 ms with the image-map sky, the +22.7 ms almost all in the shading pass (94.8 to 116.3 ms) | Renderlab's own work, not the bridge's. The bridge prepares the sky once per content (one preparation, upload and `set_sky` in a 180-frame capture), and the radiance cube's face size does not matter (faces 16, 32 and 64: 171.2 to 172.7 ms); Renderlab's suspects are in its `docs/design-sky-map-cost.md`. The rest is glossy reflections and its GI cache | Yes | Renderlab build thread |
| Large levels on the non-RT path | Outpost renders on both paths. Renderlab's non-RT shadows slow sharply with caster triangles (on the Radeon 760M, Outpost's ten million foliage triangles outlived the driver's 10 s timer; 2 million render at 406 ms a frame, 4 million do not), so non-RT foliage keeps at most 2 million triangles and fits the 32-bit ids of those shadows; the rest is reported (`ARA_RENDERLAB_LIMIT`). Alpha cuts hold at most four times their source triangles | Non-RT virtual shadows that scale with caster triangles, and 64-bit ids, in Renderlab; foliage levels of detail | Yes | Renderlab (shadow scaling, 64-bit ids); Linux thread (bridge budgets) |
| More than 256 mesh-material groups | Past 256, opaque static groups that share a material merge into one world-space batch per material, fewest objects first (`batch.rs`); a batch whose members move is rebuilt each frame. Draws are left out only when merging still does not fit (more distinct materials than the limit). Terrain tiles take at most half the groups. Both report `ARA_RENDERLAB_LIMIT` | Renderlab#2 raised `TRACED_DRAWS` from 64 to 256 at no cost when unused on non-RT and 1 to 2% on RT (Radeon 760M). Beyond that: growable per-draw tables in Renderlab, or its virtual geometry | No: nothing is left out below 256 distinct materials | Linux thread |
| Terrain and foliage levels of detail, foliage wind, terrain layer normal and roughness maps, more than four layers | Foliage density follows the user's graphics tier as in Ara's renderer (`ExternalView::foliage_percent`, the same stable selection). Resident tiles draw as full-resolution meshes (at most 128: Outpost leaves 16 of 144 out). Layer base-colour maps blend by the splat weights (`splat.rs`: one 2x2 atlas, the weights in the metallic-roughness slot, world-metre repeat from each layer's uv transform); layer normal and roughness maps are not sampled. Foliage instances draw their rule's model parts, standing still, at most 60,000 parts | Renderlab mesh LODs or its virtual geometry for distance and the tile cap; a vertex hook for wind; more texture slots for layer normals | Yes | Linux thread |
| A sky light strength other than 1 | The sky lights the scene as brightly as it looks (HDRI and gradient skies are drawn and light the scene) | Separate light and background scales in Renderlab's `Sky::Map` | No | Unassigned |
| VFX graph effects | Library effects draw on Renderlab emitters (`effects.rs`): rates and first-tick bursts, point, sphere, box and cone spawns, per-axis velocity, gravity, drag, curl noise, one vortex and one attractor, a bounce plane, four-key colour and size curves, Ara's procedural sprite sheets played over each life (fire with its heat ramp, twice as tall), camera-, velocity- and ground-aligned sprites, and emitter lights as point lights (they light the scene too; Ara's light only particles). New effects start prewarmed to the presented tick; steps follow simulation ticks. Reported and left out: mesh-surface spawns, depth collisions, ribbons and mesh particles, events, material flipbook atlases, smoke lighting gradients, velocity shutter, more than 2,048 particles per emitter. Lit smoke reads paler and drifts less than Ara's (Renderlab's near-isotropic particle lighting) | Ribbons, mesh particles, depth collisions and events in Renderlab's emitters; a phase function for lit particles; multi-workgroup sort past 2,048 | Yes: ribbons, mesh particles and events | Linux thread |
| Decal emission and ambient occlusion, distance fade, renderers that refuse decals, decal maps above 512 texels | Base colour, roughness, metalness and normal are projected; the rest is left out, every opaque surface in the box receives, larger maps are reduced | Emission and occlusion channels and a per-object mask in Renderlab's decals; fade in the bridge | Yes: per-object opt-out | PC integration thread |
| macOS and Metal | Ara's renderer draws (`ARA_RENDERLAB_UNAVAILABLE`); the Renderlab path has never rendered on Metal. What excludes it: (1) the bridge is compiled out on Apple (`cfg(not(target_vendor = "apple"))` on every module, on the `renderlab`/`wgpu`/`ash` dependencies and on `tests/render.rs`, so the Mac runs 0 GPU tests); (2) Renderlab opens `wgpu::Backends::VULKAN` only. Against wgpu 30's Metal backend: binding arrays with non-uniform indexing need MSL 3 and argument-buffer tier 2, 64-bit atomic min/max need Apple9 (or Apple8 with Mac2), `SHADER_PER_VERTEX` needs Apple10 (the M5 has it), ray queries need macOS 15, and `EXPERIMENTAL_RAY_HIT_VERTEX_RETURN`, which the RT path requires, is never offered on Metal. Renderlab's development-only Vulkan program compiler (`program_vulkan.rs`) and the bridge's shared-image handoff are Vulkan-only (the bridge falls back to its CPU copy) | Renderlab: open Metal and fetch hit vertices from its own buffers instead of hit-vertex return; then lift the bridge's `cfg` and run `tests/render.rs` on the M5. Or port Renderlab's passes onto `ara-gpu` (D35; the Mac GPU thread's paused draft Renderlab#3) | Yes | Mac ara-gpu thread (paused tonight) |
| Editor Scene view and multi-view | Not routed: one view per engine. The play process's presented image (windows, the embedded Game view, captures) works | More than one view per Renderlab `Engine` | Yes | Unassigned |
| Entity marks in captures, debug view modes, rendering debug switches, the path-traced reference | Captures have no legend; debug views and the reference fail with `ARA_VIEW_MODE_UNAVAILABLE`; switches are reported and ignored | An ID target and view modes in Renderlab's `Engine` output | Yes: agents and the editor pick through them | Unassigned |
| Automatic exposure: Ara's brightness-dependent compensation curve and adaptation speed | The bridge meters the displayed image as Ara does: the log mean between the look's low and high percentiles, steered to middle grey, plus its exposure compensation, within its EV100 limits (Outpost: within 0.1 stops of Ara). It settles at its own pace and ignores the compensation curve | The compensation curve over the metered EV; Ara's adaptation rate per second instead of a fixed half-error step; an `Exposure::Auto` in Renderlab would save the readback | Yes | Linux thread |
| Tone mappers, authored LUT textures, depth-of-field strength | Ara's grade (white balance, contrast, saturation; lift, gamma, gain) is baked into Renderlab's colour LUT in Ara's order (`grade.rs`); the vignette is matched at a 16:9 corner but darkens after the grade, so its corners lose the lift's tint. Renderlab's filmic curve, exposed so middle grey matches Ara's curves; bloom and motion blur follow the look. Depth of field maps to Renderlab's physical lens with the same focal length and circle of confusion as Ara's, but blurs about half as much as Ara's renderer on `levels/dof.scene` (the gathers differ); separate near and far strengths and authored LUT textures are reported | Hand the authored LUT image to external renderers; compare the two DOF gathers; Neutral and AgX curves in Renderlab | Yes | Linux thread |
| Sky-atmosphere luminance factors and clouds, fog volumes, volumetric fog emission | Authored `SkyAtmosphere` and `HeightFog` map to Renderlab's medium and fog value for value (`atmosphere.rs`); the artistic factors, the cloud layer, fog volumes and fog emission are reported and left out | Luminance factors and clouds in Renderlab's atmosphere; local fog volumes need a renderer feature | Yes | Linux thread (landed in #13; the PC thread has an overlapping mapping) |
| Alpha masks on skinned meshes, transmission that varies over a surface | A masked material is cut into its geometry at load with Renderlab's `cutout` (the mask averaged to the cut grid's scale, so detail finer than a cell counts by coverage; a mask that hides nothing is not cut); cut meshes keep their normal maps on tangents derived from their uv mapping (`cutout.rs`). A blended material, by alpha factor or by its map's alpha, draws in Renderlab's transparent pass. Skinned masked primitives draw opaque; uniform transmission is glass | Cutting skinned primitives; per-texel transmission needs a renderer feature | Yes | Unassigned |
| Emissive maps on see-through materials or with their own texture coordinates | An opaque material's emissive map glows per texel: the map rides in its base-colour alpha and its metallic-roughness red and alpha (`texture::with_glow`), and global illumination receives the map's mean. Those two cases do not glow, reported. In an emissive surface's own reflections other emissive objects glow with its strength | An emissive texture slot in Renderlab's material; its own parameters at reflection hits | Yes | Renderlab build thread (an emissive slot, parameters at reflection hits) |
| Material-graph materials, vertex paint, second UV set, AO maps | Base factors and maps on the standard program | Compile Ara material graphs to Renderlab WGSL programs | Yes | Unassigned |
| Rect, disc and tube lights; IES profiles | Wide spot or sphere of the same intensity; no profile | Area-light and profile support in Renderlab's light types | Yes | Unassigned |
| Texture compression and streaming | Textures decode to RGBA8 with full mip chains (about 5x the BC7 memory) | BC7/BC5 upload in Renderlab's texture path | Yes, for shipping memory | Renderlab build thread |
| Meshes with degenerate tangents | Normal map dropped for that mesh (reported by name); cut meshes derive theirs from the uv mapping (`cutout.rs`) | The same derivation for uncut meshes whose authored tangents are unusable | No | Unassigned |
| Single-sided materials | Every material is double-sided | Renderlab's shadowed paths need double-sided materials today | No | Renderlab build thread |
| Skinning with more than four influences | The four strongest, renormalized | Eight-influence skinning in Renderlab | No | Renderlab build thread |
| 2D | Unchanged: `Presenter2d` keeps Ara's 2D renderer | Nothing for 3D work | No | None needed |

Not a Renderlab gap either: Outpost's ground at dusk. Ara's renderer lights the compound
with direct sun, but the sun stands 4 degrees up behind a 67 m dune 184 m away (20 degrees
up from the compound), so Renderlab's shadow there is physically right and Ara's
renderer misses the dune's shadow. A sunlit dusk needs the sun moved in the sample.

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
  `levels/effects.scene` (campfire, smoke, sparks, swirl), `levels/graded.scene` (the crowd
under a post-process volume that grades every control), `levels/dof.scene` (depth of
field at 3 m, f/0.8). From that folder:
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
  player's GPU cache in `renderlab/` and is saved during compilation (Renderlab P-PIPE).
  On the Radeon 760M (RADV) Mesa's own shader cache already makes restarts fast: Showcase
  starts in 30.2 s (RT) and 33.9 s (non-RT) with every cache cold, and 15.8 and 19.6 s
  once Mesa's cache is warm, whether or not Renderlab's file exists. Of that, the engine
  takes 0.2 s to create and 0.3 s to its first frame; the rest is the capture settling
  49 frames at about 0.3 s each on that iGPU. On NVIDIA the Renderlab cache is what
  matters (Renderlab#15).
- A Renderlab program gets its material's texels in `surface` only. At reflection hits
  on other objects (shaded with the reflecting surface's program) and at GI hits a
  lookup returns something else: the emissive programs read their map in `surface`,
  pass global illumination a plain parameter, and glow only on a material marked in
  its base-colour alpha (`GLOW_MARK`).
