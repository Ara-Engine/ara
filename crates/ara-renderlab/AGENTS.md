# ara-renderlab

Renderlab (`github.com/Ara-Engine/renderlab`) is Ara's only renderer (D35, D39, D40; `ara-render` is
deleted). `Renderlab` implements `ara_runtime_render::ExternalRenderer`; a host installs it with
`Presenter::set_external_renderer`, and presents, captures and perf runs draw the extracted
`RenderFrame` through Renderlab's `Engine`. The player takes `--renderer renderlab-rt|renderlab-nonrt`
(or `ARA_RENDERER`) to pick the path. The editor's Scene view hosts an engine in its own process
(`ara-editor`'s `studio/viewport/renderlab.rs`: picture, object ids, clicks, outlines, hover and
grid). A Renderlab failure is an error, not a switch: no host falls back to another renderer.

This is the only crate that names a Renderlab or wgpu type. Keep it that way: the
GPU layer under Renderlab is being measured against `ara-gpu`, and a swap must touch
this crate alone.

## API

- `Renderlab::new(&Options)`: Renderlab's engine on its own Vulkan device, or on Apple on the
  presenter's Metal device (ara-gpu backend, RT only). `ARA_RENDERLAB_UNAVAILABLE` is what a failed start reports; `ARA_RENDERLAB_FAIL_START=1` forces it, to test a host's failed-engine state.
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
- `sky.rs`: the frame's sky on either engine; a sky image prepared off the frame loop.
- `rl.rs`: the mapping's `Engine`: wgpu's, or on Apple the backend's (the `backend-ara` feature of the same Renderlab revision; the root manifest patches its ara-gpu to ours).
- `meter.rs` (on Apple: a readback on metered frames, binned by the pass's CPU twin): the exposure meter's look at a frame: a compute pass on Renderlab's device bins the output's log luminance (512 bins), mapped on a later frame so no frame waits for it. The bridge takes the readings at the start of each frame, when the previous frame's (which it waited for) is done, so a cut's snap lands one frame on, as it did with a blocking read; a pass stuck past 8 frames is reported and gets a new slot.
- `environment.rs`: HDRI and gradient skies prepared for `Sky::Map` (CPU, once per sky, on a
  worker thread: 0.4 s for the gradient sky, 1.4 s for a 2k HDRI on twelve threads).
- `decals.rs`: `DecalView` to `Engine::set_decals`.
- `effects.rs`: `VisualEffect`s (Ara's VFX library) to Renderlab emitters, their sprite
  sheets from Ara's cooked atlas, and their lights.
- `terrain.rs`: resident terrain tiles and foliage instances as meshes; `splat.rs`:
  the tiles' layer maps (atlas, weights and the splat program).
- `bridge.rs`: the frame loop and the image handoff; `shared.rs`: the shared ring.
  `bridge_metal.rs`: Apple's loop, on the presenter's device (nothing crosses); backend refusals go to `gaps`.
- `id_target.rs`: the id texture Renderlab writes and its CPU readback; `Scene::tag` and `Terrain::tag` (`scene.rs`, `terrain.rs`) give objects the editor's ids.
- `gaps.rs`: `ARA_RENDERLAB_UNSUPPORTED`/`ARA_RENDERLAB_LIMIT` reporting.

## Image handoff

Renderlab renders on its own device (D35: for weeks, until its passes move onto
`ara-gpu`). On Windows the frame crosses through a ring of images the two devices
share (`shared.rs`): Renderlab's device exports them exactly as `ara-gpu`'s
`ExportRing` does, wgpu copies the output into slot 0, and the presenter's device
copies it out with `ImportedSurface::copy_out_image`. With `Options::scene_ids` the view's object ids
(`id_target.rs`: the packed nearest-opaque ids in an RGBA8 texture) cross as well: a second ring carries them
(`set_id`, copied in `publish`, read with `deliver_id` into `PresentedId` before the next publish), or
the CPU copy reads them back (`ExternalFrame::id_texels`). `ARA_RENDERLAB_SCENE_IDS=pattern` stages a
synthetic pattern through `stage_pattern` to test the ring.
Elsewhere, or when the two
devices are different adapters, the frame is read back and uploaded (the CPU copy,
3.3 to 4.1 ms at 1080p); the bridge prints once why. `ARA_RENDERLAB_HANDOFF=copy`
forces the copy. `PresentStats::external_handoff_ms` reports the cost either way.

## What the Renderlab path does not render yet

Renderlab is the only renderer (D40), so each row is something `ara-render` drew that this path does
not, or a cost, and none of them keeps `ara-render` alive. The last column names the thread working
each row: take an unassigned row, or ask its owner, so two threads never map the same thing. Each row
reports `ARA_RENDERLAB_UNSUPPORTED` once (`ARA_RENDERLAB_LIMIT` for the limit) when a frame uses it.

| Feature | On the Renderlab path today | What it would take | Owner |
|---|---|---|---|
| Frame cost | Settled frame, RTX 4070 SUPER (PC measure, 2026-11-12): Showcase 9.6 ms, Outpost 26 ms, of which Outpost's `visibility` pass alone is 25 to 27 ms on RT (the scene's triangle count and the hardware-RT traversal, not the bridge). Earlier, on Showcase at 1080p (RTX 4070 SUPER, Renderlab `4652ea3`): 33 ms (RT) and 62 ms (non-RT) against 3.0 ms for `ara-render` at Epic; about 15 and 50 ms with `ARA_RENDERLAB_SKY=constant`. Radeon 760M, Renderlab `f820af9`, RT: 150.0 ms with a constant sky, 172.7 ms with the image-map sky, the +22.7 ms almost all in the shading pass (94.8 to 116.3 ms) | Renderlab's own work, not the bridge's. The bridge prepares the sky once per content (one preparation, upload and `set_sky` in a 180-frame capture), and the radiance cube's face size does not matter (faces 16, 32 and 64: 171.2 to 172.7 ms); Renderlab's suspects are in its `docs/design-sky-map-cost.md`. P-SKY (Renderlab#16, pinned) halves the map's shading premium on the Radeon: filtered cube-array lookups and a miss-only sky lookup, 16.5 to 8.5 ms on RT and 17.4 to 9.4 ms on non-RT in Renderlab's Showcase-like scene, and 4 to 5 ms off its atmosphere sky; the rest is the lookups themselves. The rest of the frame is glossy reflections and the GI cache | Renderlab build thread (P-SKY by the Linux thread, awaiting the RTX timing) |
| Large levels on the non-RT path | Outpost renders on both paths. Renderlab's non-RT shadows slow sharply with caster triangles (on the Radeon 760M, Outpost's ten million foliage triangles outlived the driver's 10 s timer; 2 million render at 406 ms a frame, 4 million do not), so non-RT foliage keeps at most 2 million triangles, nearest instances first and after the levels of detail below, and fits the 32-bit ids of those shadows; the rest is reported (`ARA_RENDERLAB_LIMIT`). Alpha cuts hold at most four times their source triangles | Non-RT virtual shadows that scale with caster triangles, and 64-bit ids, in Renderlab; foliage levels of detail | Renderlab (shadow scaling, 64-bit ids); Linux thread (bridge budgets) |
| More than 256 mesh-material groups | Past 256, opaque static groups that share a material merge into one world-space batch per material, fewest objects first (`batch.rs`); a batch whose members move is rebuilt each frame. Draws are left out only when merging still does not fit (more distinct materials than the limit). Terrain tiles take at most three quarters of the groups (192; each tile is its own group, never merged: its own weights texture). Both report `ARA_RENDERLAB_LIMIT` | Renderlab#2 raised `TRACED_DRAWS` from 64 to 256 at no cost when unused on non-RT and 1 to 2% on RT (Radeon 760M). Beyond that: growable per-draw tables in Renderlab, or its virtual geometry | Linux thread |
| Terrain and foliage levels of detail, foliage wind | Foliage density follows the user's graphics tier as in Ara's renderer (`ExternalView::foliage_percent`, the same stable selection). Resident tiles draw as full-resolution meshes (at most 192, one group each: Outpost has 144 resident tiles; past 192, merge neighbouring tiles into chunk meshes with one weights atlas, a gutter of one replicated texel, rebuilt per chunk as tiles become resident). Layers shade as Ara's `terrainSurface` does (`splat.rs`): colour maps flattened against their tile-scale mean under Ara's breakup noise, side projections on steep slopes, and roughness (metal/roughness green) and normal maps within the user's terrain detail distance (`ExternalView::terrain_detail`), faded beyond it as Ara fades them; metalness uses its map's mean; occlusion and emission maps, rotated maps and negative scales are reported. A layer whose material has not resolved stays on the defaults without shifting the layers after it; a map that cannot be decoded costs that map only. Decoded 512² quadrants are cached by material digest and slot, so a changed detail tier re-uploads the atlases without decoding the maps again; the first build still decodes each map at full size on the frame thread. Ara's terrains hold at most four layers, as the splat does. Foliage instances draw their rule's model parts, standing still, at most 60,000 parts, each at the cooked level of detail (`Primitive::lods`) whose error projects under `ExternalView::lod_pixels` and not at all under `min_pixels` (`Terrain::choose_lods`: re-chosen when the eye moves 2 m, 15 % hysteresis; Outpost's 7,663 parts draw 6.7 million triangles instead of 13.7 million). Tiles and ordinary draws still draw their finest level | Renderlab mesh LODs or its virtual geometry for the tile cap and ordinary draws; a vertex hook for wind | Linux thread |
| A sky light strength other than 1 in fog | The sky lights the scene at `Environment.skyLight` and is drawn at 1: HDRI and gradient skies through `Sky::Map`'s intensity with `Background::ScaledSky` (Renderlab#17), the physical atmosphere through `Settings::sky_light` (#21). On `levels/skylight.scene` (0.25) the sky's brightness is unchanged on all three renderers and the sky-lit shade falls to 0.78x on Ara's renderer and 0.68x on Renderlab's (its shade holds more sky light). The atmosphere's fog keeps its full sky light, which Ara scales too | Sky light in Renderlab's fog in-scattering | Linux thread |
| VFX graph effects | Library effects draw on Renderlab emitters (`effects.rs`): rates and first-tick bursts, point, sphere, box and cone spawns, per-axis velocity, gravity, drag, curl noise, one vortex and one attractor, a bounce plane, four-key colour and size curves, Ara's procedural sprite sheets played over each life (fire with its heat ramp, twice as tall), camera-, velocity- and ground-aligned sprites, and emitter lights as point lights (they light the scene too; Ara's light only particles). New effects start warmed by 2.5 s, as Ara's composite warms each effect before its first frame; steps follow simulation ticks. Reported and left out: mesh-surface spawns, depth collisions, ribbons and mesh particles, events, material flipbook atlases, smoke lighting gradients, velocity shutter, more than 2,048 particles per emitter. Curl noise acts in the effect's space, phased by birth time, and prewarms step through the forces (Renderlab#25), so plumes drift as Ara's do; lit smoke still reads paler than Ara's (Renderlab's near-isotropic particle lighting; no lighting gradients) | Ribbons, mesh particles, depth collisions and events in Renderlab's emitters; a phase function for lit particles; multi-workgroup sort past 2,048 | Linux thread |
| Decal emission and ambient occlusion, distance fade, renderers that refuse decals, decal maps above 512 texels | Base colour, roughness, metalness and normal are projected; the rest is left out, every opaque surface in the box receives, larger maps are reduced | Emission and occlusion channels and a per-object mask in Renderlab's decals; fade in the bridge | PC integration thread |
| macOS and Metal | Renders through Renderlab's ara-gpu backend on the presenter's own Metal device (`bridge_metal.rs`; RT only): `tests/render.rs` passes on the M5. Skies go through `Sky::Map` as on wgpu (`sky.rs`, shared); normal maps draw. Skinned meshes, the colour LUT and volumetric fog draw. It is the Mac's default renderer. At 1280x720 on the M5 Pro (Renderlab `b5d835f`): Showcase 49 ms GPU, 75 ms wall; Outpost 177 ms GPU, 240 ms wall a frame. At Ara's exposure Showcase's grade matches `ara-render`'s. The backend refuses, and the bridge reports: particles, decals, textures on transparent materials; the meter reads the image back on metered frames | Those features in order (decals, particles, textured transparency), then the per-pass GPU cost on Metal | Mac ara-gpu thread |
| Editor Scene view | Done on one engine with one view (D40): the editor hosts Renderlab and draws the Scene from the editor's camera. Object ids are the draw's index in `frame.draws` plus one, then the resident terrain tiles (`Scene::tag`, `Terrain::tag`); the engine's id image feeds the overlays (`IdSource::Packed`: outlines, hover, grid) and a click asks `request_ids` (`ExternalRenderer::pick_id`). The Scene shows Lit only: its Draw Mode menu (view modes and the rendering debug switches), the View > View Mode and Rendering Debug menus and the Scene quality chooser are removed, not disabled | Add them back with Renderlab's `ViewMode`s and effect switches; see below | Mac editor thread (draw modes) |
| Entity marks in the player's captures, debug view modes, rendering debug switches, the path-traced reference | The editor's Scene captures carry marks from the engine's ids. The player's labelled capture has none: it installs the engine with `Options::scene_ids` off, so no ids cross, and it prints no legend. Debug views fail with `ARA_VIEW_MODE_UNAVAILABLE`; switches are reported and ignored. The path-traced reference is not produced: `ara-pathtrace` is deleted | The player turns ids on for captures and builds the legend from `id_texels` or the ring; wire Renderlab's `ViewMode`s to the debug views. A path-traced reference would be a new renderer, not the deleted crate | Unassigned |
| Automatic exposure: Ara's HDR histogram and highlight headroom | The bridge meters the displayed image as Ara does: the log mean between the look's low and high percentiles, steered to middle grey, plus its compensation and Ara's compensation curve at the metered scene EV100, within its EV100 limits; it adapts at the look's rate over a fixed 1/60 s per rendered frame, as Ara's renderer does (no tick or wall clock, so a paused game still adapts), and snaps after a cut, as Ara's history reset does, until it settles or is pinned at a limit. It meters every frame while snapping and one in 16 while adapting, toward the last target in between. The look is a GPU histogram of the output (`meter.rs`) that arrives by the next frame and is never waited for: no frame blocks on the meter, on either route (Outpost: within a quarter stop of Ara). Ara meters the HDR scene with a highlight headroom; the bridge meters the tone-mapped image | An `Exposure::Auto` in Renderlab: an HDR histogram with Ara's headroom, and no readback | Linux thread |
| Depth-of-field strength | Ara's grade (white balance, contrast, saturation; lift, gamma, gain) is baked into Renderlab's colour LUT in Ara's order, gain before gamma (`grade.rs`); the vignette is matched at a 16:9 corner but darkens after the grade, so its corners lose the lift's tint. Ara's tone curve (PBR Neutral, Hill's Filmic, AgX and its looks, the ACES fit) rides in the same table, undoing Renderlab's (Showcase, Neutral: mean display luminance 0.1828 against Ara's 0.1823, before 0.2241); scene values past about 9 after exposure, where Renderlab's curve saturates, all read as its brightest entry; bloom and motion blur follow the look. Depth of field maps to Renderlab's physical lens with the same focal length and circle of confusion as Ara's, but blurs about half as much as Ara's renderer on `levels/dof.scene` (the gathers differ); separate near and far strengths are reported. A look's authored LUT texture rides last in the same table, on the sRGB-encoded colour and blended by its contribution, as Ara applies it | Compare the two DOF gathers | Linux thread |
| Sky-atmosphere luminance factors and clouds, fog volumes, volumetric fog emission | Authored `SkyAtmosphere` and `HeightFog` map to Renderlab's medium and fog value for value (`atmosphere.rs`); the artistic factors, the cloud layer, fog volumes and fog emission are reported and left out | Luminance factors and clouds in Renderlab's atmosphere; local fog volumes need a renderer feature | Linux thread (landed in #13; the PC thread has an overlapping mapping) |
| Alpha masks on skinned meshes, transmission that varies over a surface | A masked material is cut into its geometry at load with Renderlab's `cutout` (the mask averaged to the cut grid's scale, so detail finer than a cell counts by coverage; a mask that hides nothing is not cut); cut meshes keep their normal maps on tangents derived from their uv mapping (`cutout.rs`). A blended material draws in Renderlab's transparent pass only when it is seen through: an alpha factor below one, or a map with partial coverage; a blended map with no alpha is opaque and a there-or-not one is cut (`cutout::alpha`), because each transparent object costs Renderlab's visibility and transparency passes (about 4 ms on Showcase's RT path for two lenses). Skinned masked primitives draw opaque; uniform transmission is glass | Cutting skinned primitives; per-texel transmission needs a renderer feature | Unassigned |
| The blend class of a base-colour map (`cutout::alpha`): opaque, there-or-not (cut) or partly transparent (blended) | Counted over the whole map, not the texels a mesh uses, so an atlas shared by a bush and a window gets one class; thresholds are constants (15% partly transparent blends, over 3% on a cut map is reported); no per-material override. A run-time alpha factor crossing 1.0 pops a there-or-not map between the cut path (opaque, hard edge, cut mesh) and the blended path (uncut mesh, sorted, soft), re-uploading the mesh, and an emissive map stops glowing below 1 (reported then) | Count the class over the texels a mesh's uvs reach; take the author's alpha mode as an override; blend or cut by the class alone and let the factor scale coverage | Unassigned |
| First-sight decode of base-colour maps on the frame thread | `Coverage::of` decodes each blended material's map once, inline in the scene sync (about 134 MB transient for a 4096 map, per the review), and `with_glow` decodes its host textures again (17 to 50 million bilinear reads inline for a 4K emissive map on a 512 host); cached per map, unmeasured | Classify from the cooked texture's mips or on a worker | Unassigned |
| Emissive maps on see-through materials or with their own texture coordinates | An opaque material's emissive map glows per texel: the map rides in its base-colour alpha and its metallic-roughness red and alpha (`texture::with_glow`), and global illumination receives the map's mean. Those two cases do not glow, reported. In an emissive surface's own reflections other emissive objects glow with its strength | An emissive texture slot in Renderlab's material; its own parameters at reflection hits | Renderlab build thread (an emissive slot, parameters at reflection hits) |
| Material-graph materials, vertex paint, second UV set, AO maps | Base factors and maps on the standard program | Compile Ara material graphs to Renderlab WGSL programs | Unassigned |
| Rect, disc and tube lights; IES profiles | Wide spot or sphere of the same intensity; no profile | Area-light and profile support in Renderlab's light types | Unassigned |
| Texture compression and streaming | Textures decode to RGBA8 with full mip chains (about 5x the BC7 memory) | BC7/BC5 upload in Renderlab's texture path | Renderlab build thread |
| Meshes with degenerate tangents | Normal map dropped for that mesh (reported by name); cut meshes derive theirs from the uv mapping (`cutout.rs`) | The same derivation for uncut meshes whose authored tangents are unusable | Unassigned |
| Single-sided materials | Every material is double-sided | Renderlab's shadowed paths need double-sided materials today | Renderlab build thread |
| Skinning with more than four influences | The four strongest, renormalized | Eight-influence skinning in Renderlab | Renderlab build thread |
| Reflection-probe capture, the effect preview and the Game view's preview outside Play | Removed from the editor, not disabled: the Lighting page's Recapture button, the `lighting.probe.*` commands (recapture, cancel, refresh, copy state), the `render.probe.*` protocol methods and the VFX page's preview column and timeline, with the emitters' Hide and Isolate (`vfx.emitter.hide`, `vfx.emitter.isolate`), which only affected that preview. Existing probe captures keep loading. The Game tab outside Play shows a tile saying so; Play itself runs in the player and is unaffected | A reflection capture baked from Renderlab's engine; an effect preview on a hosted engine; a second camera on the Scene's hosted engine (or a second engine) | Unassigned |
| Material in-node previews and pictures for models, materials and effects in Assets | Material graph nodes draw plain; Assets tiles show a glyph for models, materials and effects (textures keep their thumbnails), so Assets > Rebuild Thumbnails only affects textures | Rendering a material, model or effect to an image on a hosted engine | Unassigned |
| The Reflection Probe component's capture settings (Bounds, Priority, Blend distance, Capture resolution, Projection, Capture offset, Intensity) | Authored and saved, ignored: Renderlab traces reflections instead of reading captures | Nothing planned; the component stays for projects that bake captures | None needed |
| Look-development lab, stress gates and the player's pass contract | `ara-dev lab set`, the reference and temporal metrics and the live parameters read the old renderer's registry, which went with it; `lab gate` has no worker (`render_v3` is deleted); the player's perf windows still expect the raster pass names (`ara_bench::passes`), so a measure reports `INVALID_PASS_SET` | Parameter keys in Renderlab; a Renderlab gate worker with the `render_v3` contract; a pass contract over `External.Pass.*` | Unassigned |
| A model piece's emissive factor | Does not change the Renderlab picture: a `MeshRenderer`'s emissive factor on a model's pieces is not applied to the engine's materials (a red and a green lamp capture differ by 0.01 of 255 on average). A locked exposure does hold: a Play started with `renderDebug.exposure` 7.798 against 10 gives mean luma 93 against 32 | Apply the factor in the bridge's material mapping | Linux thread |
| 2D | Unchanged: `Presenter2d` keeps Ara's 2D renderer | Nothing for 3D work | None needed |
| The editor's Game tab on Linux | Play in the Game view embeds the player's image through shared surfaces, which are NT handles on Windows and a Metal path on the Mac. On Linux the embed fails (`play.embed failed: ARA_GPU_EXTERNAL: shared surfaces use opaque NT handles; this platform is unsupported`): Play still runs the game in its player process, but the Game tab shows no picture (found by the Linux gate, on main before and after the old renderer's removal) | A Linux shared-surface route (dma-buf or Vulkan external memory fds) for `ara-gpu`'s export ring and the player's embed | Unassigned |
| Graphics quality settings | The user's graphics tier reaches the bridge only as foliage density (`ExternalView::foliage_percent`) and the terrain detail distance (`terrain_detail`). Resolution scale, shadow, GI and reflection quality, and the post toggles do not: the engine renders at its own settings whatever `ara graphics` or the editor's quality choice says | A tier-to-`ViewDesc` mapping (`render_scale`, per-view look) and `Settings` in the bridge | Unassigned |
| Glass clicks on Apple | On the wgpu path `pick_id` answers with the frontmost id, glass included. On Apple a click on glass picks what is behind it (found on the Mac, 2026-11-12) | The Metal port's id readback answering the `front` layer as the wgpu path does | Mac ara-gpu thread |
| The pane (transparent) pass costs per triangle | Panes draw in Renderlab's transparency pass and, with ids on, in the id pass's glass raster: both scale with the panes' triangles, so a detailed glass mesh costs far more than a flat lens (about 4 ms for two lenses on Showcase's RT path, per `cutout::alpha`). The bridge keeps blended materials to what is seen through | Renderlab's pane pass batching by instance, or the bridge cutting detailed panes into the opaque path | Unassigned |
| The cut frame on the non-RT path | A cut (a camera jump, a capture's reset) renders without temporal history and a snapped meter, so it costs more than a settled frame, most on non-RT (found 2026-11-12, not yet measured per scene; a still capture settles 49 frames at about 0.3 s each on the Radeon). The editor's Scene hits it on every jump of the camera (the bridge cuts after a turn of 45 degrees or 20 m) | History reuse across a cut in Renderlab, or a cheaper first frame | Renderlab build thread |
| Outpost's RT visibility pass | On the RTX 4070 SUPER Outpost's settled frame is 26 ms and its `visibility` pass alone reads 25 to 27 ms (run to run), against 9.6 ms for all of Showcase | Renderlab's traversal for large triangle counts (levels of detail or virtual geometry) | Renderlab build thread |

**Showcase frame cost by Renderlab branch** (Radeon 760M only; `ara capture --preset epic
--stability 30`, 1920x1080, the bridge's `External.Frame.Gpu` and `External.Pass.Shading`,
fastest of four runs on a shared machine, spread up to 10%; Ara's renderer: 58.2 ms):

| Renderlab | RT GPU / shading, ms | non-RT GPU / shading, ms | Sky map's shading premium, RT / non-RT |
|---|---|---|---|
| main pin `901c1d2` | 159.3 / 105.9 (66%) | 291.6 / 194.7 (67%) | 20.7 / 22.6 |
| P-SKY `48f86f1` (Renderlab#16) | 149.9 / 96.6 (64%) | 282.6 / 185.1 (65%) | 11.1 / 12.8 |
| P-SKY2 `d7e2096` (#19) | 150.3 / 96.1 (64%) | 281.4 / 184.0 (65%) | 7.4 / 11.0 |

The premium is map against `ARA_RENDERLAB_SKY=constant` in the same round. The RTX 4070 SUPER
before P-SKY: non-RT 40.5 ms GPU, shading 33.4 (82%), against 3.0 ms on Ara's renderer.

Not a Renderlab gap either: Outpost's ground at dusk. Ara's renderer lights the compound with direct sun, but the sun stands 4 degrees up behind a 67 m dune 184 m away (20 degrees
up from the compound), so Renderlab's shadow there is physically right and Ara's renderer misses the dune's shadow. A sunlit dusk needs the sun moved in the sample.

Reflection probes are not a gap: Renderlab traces reflections on both paths. Its glossy band could leave bright speckles on glossy surfaces before `4652ea3` (a renderer defect, since fixed there); `Options::max_reflection_roughness` (`ARA_RENDERLAB_REFLECTION_ROUGHNESS` in the player) limits tracing, 0.05 keeping mirrors only.

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
  camera, `--video 6 --video-yaw 300` for cuts (or one hard cut inside a moving clip:
  `--video 120 --video-move 4,0,-4 --video-yaw 12 --video-cut-at 60 --video-cut-move -20,0,15 --video-cut-yaw 140`).
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
  lookup returns something else: the emissive program reads its map in `surface`,
  passes global illumination a plain parameter, and glows only on a material marked in
  its base-colour alpha (`GLOW_MARK`). One program serves mapped and unmapped
  materials (`mapped` parameter): every program is more pipelines and a shading pass.
