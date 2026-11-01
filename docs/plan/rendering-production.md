# Production rendering program

Goal: Ara's raster renderer at or past Unreal 5's lighting quality, at a lower frame cost,
for any project. The Polar facility comparison (`D:\AraScratch\UECompare` against
`AraPolar`) is a yardstick, never the target: every item is general engine work with its
edge cases, tests, debug views and CLI access. Ray tracing stays an opt-in (D27, D29).

Each item lands as its own commit(s), with focused tests, a debug view where the effect can
be isolated, and a `ara capture` switch where that helps diagnosis. Status: `[ ]` open,
`[~]` in progress, `[x]` done (with the commit).

## Lighting

- [x] **Area lights.** Merged ab19eac38 (rect/disc/sphere/tube, LTC, IES, soft local shadows,
  path-traced shapes, fog through look-lights.slangh). Wide penumbrae, area-light soft
  shadows in fog and the self-shadow terminator: done with soft shadows (below). Open: rect
  source texture, emissive surfaces promoted to area lights. Original item: Rect, disc, sphere and tube lights (`LightKind`), LTC specular and
  diffuse (Heitz 2016), barn doors for rects, optional light texture (rect), IES / light
  profiles for point and spot ("shaped lights"), source radius/length softening, their
  shadows (local faces, contact-hardened by source size), GI relight and fog. Unreal:
  `URectLightComponent`, source radius/length, `IESTexture`.
  Done (2026-11-01): `Light` kinds Rect/Disc, `LightUnits` (lm/cd/nits), source radius/length,
  rect size and barn doors, `sourceAngle`; 112-byte record; LTC shading fitted to Ara's GGX
  (`examples/ltc_fit.rs`, exact per-edge horizon clipping, ellipse form factor for discs and
  spheres) in deferred, forward, ray hits, probe relight and fog; IES import (`ara.ies`) with
  profiles in raster and the path tracer; reference samples every shape with MIS hits; PCSS
  local shadows; light complexity view; editor gizmos and Create entries. Open: rect source
  textures; promoting large emissive surfaces to rect/disc lights; Unreal converter rect export.
- [ ] **Emissive surfaces light the scene.** Emissive meshes feed probe GI (hit radiance)
  and screen-space GI; large or bright emitters can be promoted to area lights for direct
  light and shadows (Lumen's emissive lighting; Unreal's "Emissive Light Source").
- [x] **Soft shadows (sun and every local light).** Branch t3/9c7b8618 (2026-11-01).
  Every shadow map (local atlas, cascades, far map) gets a moment pyramid (`look::moments`:
  RGBA32F mean, centred variance and farthest depth, packed levels, rebuilt per drawn tile);
  `shaders/look-shadow.slangh` looks up any of them: the receiver's tangent plane in map
  space (texels block only above it and nearer the light: no acne, no self-shadowing at any
  width, no normal offset; curved receivers keep the LTC horizon as their terminator), a
  blocker search over the source's widest cone (each texel counted only inside its own
  cone, weighted by map proximity), and per-texel kernels of the source's projected shape
  (rect, disc, sphere, tube; the sun disc from `sourceAngle`) summed over the 4x4 texels
  of the level the penumbra spans, blended across levels: penumbrae of any width, sharp at
  contact. Cascades and far map on Low/Medium; VSM keeps its PCSS and blends to the sun
  maps past its 4-texel tent, so presets agree. Fog (froxels and the coarse grid) filters
  the penumbra or the froxel, whichever is wider. `ViewMode::ShadowPenumbra`,
  `--without soft-shadows`. Measured (ara-runtime-render `tests/soft_shadows.rs`, 10-90%
  width vs the path tracer, Medium/High): rect +3/+3%, low rect +4/+3%, disc +7/+7%, sphere
  +4/+4%, sun 0.54 deg +5/+3%, sun 10 deg +6/+5%, tall blocker +4/+3%. Cost
  (`examples/shadow_costs.rs`, 2560x1440, frame p50 before -> after): sun only Low 0.53 ->
  0.64, Medium 0.92 -> 1.05, High 1.66 -> 1.96, Epic 2.76 -> 2.90 ms; four shadowed area
  lights covering the view Low 0.62 -> 0.84, Medium 1.20 -> 1.91, High 2.22 -> 3.13, Epic
  3.45 -> 4.86 ms. Stability (`ara capture --stability 48`): no change with
  `--without soft-shadows` on the shapes, fog-beam and Polar scenes. Open: area-light cost
  (a screen-space shadow mask pass, half-rate wide penumbrae); round blockers resting on their receiver keep a lighter umbra than the path
  tracer (their lower half is hidden from the light's centre: slope extrapolation as
  Unreal's SMRT); kernels cross cube-face edges clamped; pyramids add ~200 MB at Epic.
- [~] **Global illumination ("Lumen alternative").** Screen probe GI (D32, `look::screen_gi`,
  High/Epic, `--without screen-gi`, editor View > Rendering Debug > Screen GI, yellow in
  `--reflections sources`): 16 px probes on the G-buffer trace 64 fixed directions through a
  Hi-Z pyramid; hits from last frame's colour (minus promoted emission, capped at what a diffuse
  surface of the hit's albedo sends, as the bitmask AO's bounce), misses from the probe volume;
  3x3 tent gather; rough lobes by the view's true GGX lobe. Deterministic, no history.
  First pass (display-space regions): Polar 0.554 -> 0.468, both test rooms regressed on their
  rough-metal ceilings because the probe volume overshot (3.35x with captures off).
  Probe volume fixed (2026-11-01, 7173bc4a4, b887d3175, 2dd96a549, a9f78a7e1): measured on
  minimal rooms (`D:AraScratchgi2`: sky-lit box, rotated, closed, thick-walled, furnace,
  sensor spheres), every overshoot had a separate cause: (1) 8x8 moments blurred over a cos^12
  lobe mixed a wall with the open space past its edge, so Chebyshev passed points behind thin
  walls (a closed room glowed along every edge): now 16x16 per-texel moments from the bake's
  sub-rays, per-tap visibility, rays clamped at twice the spacing; (2) relighting rebuilt each
  texel's hit at its hits' mean plane, which floated outside the room (window texels seeing the
  ground beyond) or in a sunbeam (a block and the wall behind): now a real sub-ray hit; (3) the
  octahedral convolution weighed texels equally though axis texels cover half the mean solid
  angle; (4) shading's cascade pick treats points outside its map as lit, which relit every
  hit behind the camera: now the first cascade that maps the hit; (5) misses recorded at the
  bake's 200 m ray range hid every point on a coarse landform lattice (open ground lost its sky
  light), now at the moment limit, with visibility tested 5% of the spacing off the surface;
  (6) the backface weight came from the biased point, so probes nearer a wall than the bias
  turned away from the points in front of them: a grid of dark dots on lamp-lit walls (now
  from the true point, as DDGI). A 0.5 m fine lattice was tried and reverted (fc3604c72): once
  the relight was fixed it only helped the synthetic boxes, and its probes beside walls
  point-sampled sharp lamp (IES) patterns into blotches. Furnace tests stay within 1-5%.
  Ten scenes, High, EV-locked, linear region radiance (PBR Neutral inverted), mean |ln ratio|,
  base 4df69e86b -> now, screen GI off (on): Polar 0.266 -> 0.332 (0.182 -> 0.196); emissive
  panel room 0.084 -> 0.113 (0.263 -> 0.178); sunlit room 0.306 -> 0.228 (0.443 -> 0.343); sky
  room 0.256 -> 0.164 (0.378 -> 0.167); courtyard 0.035 -> 0.034; sky-lit box 0.598 -> 0.095
  (0.559 -> 0.139); glazed room 0.371 -> 0.092; unglazed 0.347 -> 0.067; open ground under a
  sky atmosphere, sun 10 deg 0.274 -> 0.061, sun 2 deg 1.276 -> 0.087. Mean 0.381 -> 0.127
  (off), 0.410 -> 0.149 (on), so screen GI stays off by default (D32). Area-light shapes scene
  0.160 -> 0.107, its dot grid and the IES scene's blotches gone.
  Cost at 2560x1440 (one frame, unguarded, busy PC): GI.Relight + GI.Gather Polar High 0.31 ->
  0.37 ms, sunlit room High 0.21 -> 0.31 ms, Polar Epic 0.45 -> 0.48 ms (16x16 visibility taps,
  solid angles, the cascade search). `--stability 48`: Polar High 0.8% of 16x16 blocks flicker
  (0.7-0.9% before), the sunlit room 0%, Polar Epic 39% -> 1.3%.
  Open: (a) Polar's probe-only floors and back room went darker (left floor 0.62 -> 0.38);
  screen GI recovers most of it. (b) The sunlit room: its rough-metal ceiling (captures
  0.35x, probes 2.9x, screen probes 2.5x); one sun test per 22 degree texel point-samples a
  small bright patch. (c) Screen GI:
  mottling where few of a probe's 64 rays see a window, overshoot on rough-metal ceilings
  (pyramid hits blur bright neighbours in; two levels sharper helped the panel room 0.29 ->
  0.21 but mottled more), worse than probes on open ground. (d) High's copper block reads
  black with screen GI off (black reflection capture). (e) Emissive panels as rect lights
  (Area lights item). Also open: adaptive extra probes where the 3x3 gather finds none,
  Epic's quality (tile 8 cost 3.4 ms), an in-engine region tool, Medium screen GI.
  Original item: Bounce energy that matches the path
  tracer: screen-space GI over the probe field, a per-probe radiance cache with sky and
  emissive, multi-bounce, metals; temporal stability with no noise (D27). Measured against
  `ara-pathtrace` on the samples and Polar.
- [~] **Volumetric fog and light shafts.** Done (c8a7870ec, ce532b13d, 3876cdedf, cook
  5fb204d26): `look::volumetric` near froxel volume (Medium 16 px/48 slices, High 12/64,
  Epic 8/96; Low keeps the coarse grid) to `HeightFog.volumetricDistance`, ambient
  (probe GI or sky) on a 4x4x2 grid, injection with Halton jitter, reprojected history and
  per-group light culling, Hillaire integration; `LocalFogVolume`; emissive fog, extinction
  scale, `Light.volumetricScattering`; coarse grid parallelised (0.32 -> 0.09 ms);
  `--without volumetric-fog` and editor toggles. Polar 1440p fog GPU time: Low 0.09,
  Medium 0.16, High 0.23, Epic 0.67 ms (unguarded, busy PC). Open: the reference's fog is
  itself a coarse froxel grid (no shafts; needs a per-pixel path-traced medium), local fog
  volumes in the fidelity renderer, froxel leaking through thin occluders at grazing
  angles (as Unreal), virtual shadow pages for fog, a fog buffer view. Polar's authored
  density (0.00025/m) shows faint shafts; check the UE FogDensity conversion.
  Original item: Froxel volumetric fog (Unreal's volumetric fog):
  height and local fog volumes, sun with shadows (shafts), local lights with shadows,
  emissive fog, temporal reprojection, per-light volumetric scattering intensity.
- [~] **Atmosphere parity.** Done (branch t3/190a390b): `SkyAtmosphere` v2 takes every
  Unreal SkyAtmosphere parameter in Unreal's units and defaults (planet, Rayleigh, Mie,
  ozone tent, multi-scattering, trace sample scale, luminance factors, aerial perspective
  distance scale and start depth, height fog contribution, minimum light elevation; v1
  `aerosols` migrates into the Mie scales); `Light` gains Unreal's atmosphere sun light,
  index, disc colour scale and per-pixel transmittance. One model and shader
  (`look::atmosphere`, `shaders/atmosphere-model.slangh`, a checked copy in the fidelity
  root) drives raster, hybrid and the path-traced reference: Bruneton transmittance with an
  analytic planet shadow, Unreal's default two-ray multi-scattering, a horizon-packed sky
  view, two atmosphere lights (a moon above a set sun takes the shadowed sun slot), discs,
  aerial perspective to 111 km (46 coarse slices), Unreal's global light transmittance
  (15 samples at 500 m) or per-pixel, sky light recapture on any sun/sky change.
  `ue_to_ara.js` maps it one to one. Measured against UE 5.7 MRQ (manual exposure, tone
  curve off) on Earth and Mars-like skies at sun 90/30/10/2/-2 degrees plus a moonlit
  night, 22 shots: sky bands 0.003-0.042 stops mean, chroma d <= 0.005; ground under the
  sky light 0.01-0.09 stops; aerial perspective (black boxes at 4 and 15 km) within 0-15%;
  the path tracer's sky within 0.003-0.008 stops of raster. Cost at 1440p (4070 SUPER,
  busy PC): 0.072 ms per frame (base 0.037), sky light recapture 0.150 ms on a sun change
  (0.130), LUT rebuild 0.020 ms (0.049); `--example sky_timings`.
  Open ground under probe GI now matches the sky light (GI item, b887d3175); open: the second atmosphere light lights no surfaces while the sun is up;
  the reference traces camera rays to 10 km only (no aerial perspective on farther
  geometry) and its rough-ground sun specular at low sun is weaker than raster/UE; height
  fog contribution is implemented but not measured against UE; Unreal's holdout and
  render-in-main-pass flags; cameras above the atmosphere (the sky view clamps to its top).
  Original item: Hillaire sky atmosphere (transmittance, multi-scattering,
  sky view, aerial perspective LUTs) matching Unreal's SkyAtmosphere component values.

## Surfaces

- [~] **Consistent glass.** One transmission model across raster, hybrid and path tracer:
  thin and solid glass, roughness, tint by thickness, shadows by transmittance, reflections
  of and through glass, sorting. Done (2026-11-01, branch t3/production-glass-transparency): the path tracer's
  transmission was diffuse (glass a white sheet, opaque to the sun); it is now a GGX
  dielectric (thin and volume, Beer-Lambert absorption) whose shadow rays take glass's
  straight transmittance (`look-glass.slangh` `glassTransmittance`, shared with raster);
  raster/hybrid glass casts coloured, stacked sun shadows (optical-depth translucent map,
  High/Epic), re-copies the scene where glass overlaps transparent draws behind it, refracts
  solid slabs and hollow shells (bottles) correctly, marches close or sky-backed glass, and
  shows in hybrid mirrors; probe GI bakes see through glass. Test scenes:
  `D:\AraScratch\glass` (objects, stacked panes, sunlit room, fog, mirror). Region means
  over the reference at equal EV, before -> after: solid slab 0.17 -> 0.97, top of a ball
  lens 0.54 -> 1.06 (raster) / 0.93 (hybrid), bottle 1.40 -> 0.78, glass shadows 2.2x ->
  0.94-1.10 (High), three stacked panes 1.47 -> 1.00, sphere behind a pane 0.65 -> 0.94,
  bounce in a glazed room 2.7-3.0x -> 1.9-2.1x (the unglazed room's GI bias is 1.7-2.0x), then
  1.0-1.2x once the probe volume stopped leaking (GI item: glazed 0.108, unglazed 0.086).
  Open: Low/Medium cast no glass
  shadows; the ray-traced fidelity renderer (editor/Play with the ray-tracing opt-in) still
  shows glass dark and noisy; local-light shadows ignore glass; dispersion
  (KHR_materials_dispersion needs a material word); raster ball lenses approximate the
  far side by a sphere fit (hybrid traces it); glass in screen-space reflections.
- [~] **Screen-space reflections.** Hi-Z traced (look-screen.slangh `hizTrace`: closest-depth
  pyramid, crawls on behind thin foreground), then captures and probes; rough lobes past 0.25-0.4
  hand over to screen probes (D32). SSR.Trace 0.60 ms at 2560x1440 High. Open: per-pixel cone
  footprints from the Hi-Z cell, measured before/after cost at equal settings.
  Original item: Hi-Z traced SSR (min-depth pyramid), stochastic rough
  lobes with a deterministic, noise-free resolve, then captures and probes.
- [x] **Vertex painting.** Vertex colours imported (glTF `COLOR_0`, FBX), stored per
  mesh, editable with an editor paint tool, readable in material graphs (layer blends).
  e3f51b456 (import through render, `vertexColor()` paint layers, per-instance `VertexPaint`),
  fa146c6d6 (Mesh Paint mode). Open: the Polar floor's layered UE master materials need
  Ara graphs; virtual geometry (ARAG) carries no colours yet.
- [x] **Decals.** Deferred decals (Unreal's `DecalComponent`): box projection, base colour,
  normal, roughness/metal, emissive, fade, sort order, receivers filter. cb5fda5ee
  (tiled G-buffer decals), fa146c6d6 (Create > Decal, gizmo). Open: a specular G-buffer
  channel for decal specular; Metal's storage alias view; Polar decal conversion.

## Tools

- [x] **ara-render GPU test hang.** NVIDIA's vkDestroyDevice deadlocks while another device in
  the process holds a queue waiting on the host; test gates now go through
  `ara_gpu::vulkan::QueueGate` (56c1d9df1, merged ab19eac38): 20/20 runs at 16 threads.

- [~] **Editor views: no noise, full quality, correct sizing.** Done (merge b1fb2bfd0): Game
  frames never drawn small or stretched (cover the panel until the new size arrives), whole
  physical pixels at fractional DPI, native-resolution embedded Game view (+~1 ms GPU),
  exposure finishes adapting before a still Scene stops (120-frame cap), single resize per
  change, HUD at the exact scene size; `ara-editor --verify-game-view` route. Open:
  (1) automatic reflection captures finish baking ~1 s after Play entry (brightness 49 to 45
  over 15 frames); waiting for them or baking all at once both failed; try a bounded number
  of bake steps per frame during entry, without the lighting-change re-bake. (2) After a
  camera move, Polar's Scene exposure drifts ~0.2%/frame for 240+ frames (4.90 to 4.08;
  D:\AraScratch\presentation\evidence\final2\scene\scene-quality.json): check the
  adaptation rate the Unreal speeds map to and decide the editor policy (finish adaptation
  quickly at rest). Standalone games still upscale (unchanged cost).

- [ ] **Capture exposure defaults.** A scene with no post-process volume captures at the
  legacy manual exposure 0.0015 (EV 9.1, daylight), so lamp-lit scenes come out ~4.5 stops
  dark unless `--ev100` is passed, and the capture JSON reports `"exposure": 0.0015` even
  under `--ev100`. Report the exposure actually applied, and default captures without a
  volume to the same metering the editor and Play use.

- [ ] **Stale player under `ara capture`.** `ara capture` renders through the sibling
  `ara-player` binary, and building only `-p ara` leaves an older player that loads the
  tree's new cooked shaders against its old data (an old CPU probe bake drew star patterns
  over every wall). Refuse a player built from different sources, as `ara-dev` already
  does for the editor's companion.

- [ ] **Cooker determinism.** An incremental re-cook of unchanged sources rewrote Metal
  metadata (`ResourceCount` 31 to 224 on fullscreenVertex) relative to the fresh cohort cook.

- [x] **C# graphics settings.** Typed per-user presets/scalability, live renderer/display
  application, explicit RT restart, persistence, C# events and engine protocol/CLI
  (`a63467bc7`; [API and application boundaries](../dev/graphics-settings.md)).
- [~] **Debugging.** Done: `ara capture --reference [spp] --bounces n` (path-traced ground
  truth, 2ef3a3419), `--ev100` exposure lock (2ef3a3419), `--view` buffer visualization
  (510d884ca, 0a864f5c5). Open: editor toggles for each, region luminance tooling in-engine.
  Original item: A debug view for every effect above (`ViewMode`, `RenderDebug`),
  `ara capture` switches for each (`--view`, `--without`), a path-traced reference capture
  (`ara capture --reference`), and region luminance/compare tooling. Done: `--view` for
  every view mode, vertex-colour and decal-coverage views (7d53e77ee).
- [x] Per-mesh Cast Shadows (3f2a35625); horizon shadows past the maps' reach (e3f974ddd).
