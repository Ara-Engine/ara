# Production rendering program

Goal: Ara's raster renderer at or past Unreal 5's lighting quality, at a lower frame cost,
for any project. The Polar facility comparison (`D:\AraScratch\UECompare` against
`AraPolar`) is a yardstick, never the target: every item is general engine work with its
edge cases, tests, debug views and CLI access. Ray tracing stays an opt-in (D27, D29).

Each item lands as its own commit(s), with focused tests, a debug view where the effect can
be isolated, and a `ara capture` switch where that helps diagnosis. Status: `[ ]` open,
`[~]` in progress, `[x]` done (with the commit).

## Open at the pause (2026-11-01)

Work stopped here for an engine overhaul. Everything below is open; the item sections
hold the evidence and causes.

- **Glossy floor reflections show the wrong content** (Polar's corridor floor): Ara
  reflects overhead frames where the reference shows the ceiling skylights. Blur is fixed
  (0dd2d6068); the cause is the fallback sources missing off-screen content (see
  Reflections and the planar-reflection checkpoint).
- **Screen probe GI ships off** (D32): it loses to probe-only GI on Polar, the sky room,
  the sky-lit box and open ground (diffuse excess there).
- **Probe GI near-field:** floors within 0.6 m of a lit wall read 1.2-2.3x; a per-probe
  irradiance gradient is parked in D:AraScratchgi3parked.
- **Bounce albedo:** the bake adds specular albedo to full diffuse (about 4% per bounce
  over the path tracer's (1-F) diffuse); correcting it darkens the glass rooms 8%, so
  they miss light elsewhere.
- **SSR-to-probe step** on glossy floors under lamp pools (probes read 1.10-1.24x).
- **Rough-metal ceiling** in the sunlit room reads 0.67x against the corrected reference.
- **Polar region error** 0.192 (screen GI off); every other test scene is within ~13%.
- **UE comparison scene:** shafts faint at the authored fog density (check Unreal's
  FogDensity conversion); UE vertex-paint frost/wet layers and decals not converted.
- **Play entry:** reflection captures finish ~1 s after Play starts; Scene-view exposure
  drifts after camera moves (Tools).
- **Cooker determinism** (Tools); **quiet-machine timings** for every item measured
  under load today.
- Fog: faint banding in fog ambient from probe interpolation; the reference leaves out
  extinction toward local lights; local fog volumes are not in the fidelity renderer.

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
  textures; Unreal converter rect export. Emissive panels become rect/disc lights (2026-11-01,
  ebc298f74; next item).
- [~] **Emissive surfaces light the scene.** Done (ebc298f74): the emitter search
  (ara-runtime-render `emitters.rs`) turns flat, evenly emitting faces of 0.01 m2 or more that fill
  a rectangle or disc and escape their own mesh into one rect or disc light of their luminance, a
  centimetre off the face (LTC, soft shadows, true size); fixtures and uneven faces keep clustered
  spot/point lights. Rect and disc lights shadow through one 120-degree face along their normal (a
  cube's side faces saw the wall behind a panel and drew polygonal patches on grazing receivers);
  penumbra table unchanged. The panel room's four hotspots are gone. Open: rect source textures
  (uneven panels), the panel room's back wall still 1.56x (probe GI, below).
  Original item: Emissive meshes feed probe GI (hit radiance)
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
  Artifact pass (2026-11-01, branch t3/478a28ed, scratch `D:\AraScratch\gileft`): (1) the
  area-light scene's floor rectangle was SSR's hit region on a rough floor: one mirror ray's
  binary hit/miss printed the mirrored wall's outline wherever the probe fallback disagreed by a
  few percent (captures played no part). `SSR.Resolve` (04becdb8b) spreads each half-resolution
  pixel's confidence and colour over its lobe's footprint as seen through the mirror (GGX median:
  4 t alpha in the plane of incidence, N.V of that across, t bounded by the view distance, same
  surface and normal only): 0.116 ms Polar High at 2560x1440. (2) The IES wall's smudge was probe
  GI: each 22-degree probe texel was relit at one sub-ray hit, so probes point-sampled the profile's
  spokes. Relight now lights each texel's hit plane over its footprint (a0b08334c): local lights at
  4x4 points for profiles and hard spot cones, 2x2 otherwise, every shadow once as PCF over the
  footprint disc on the moment pyramid; the sun keeps VSM pages first (cascades alone read the
  glass rooms 15% dark, unexplained). GI.Relight Polar High 0.217 -> 0.294 ms (1280x720). (5)
  Reflection captures reflected black on everything rougher than a mirror since their first
  commit: `capturePrefilterMain` wrote level k at captureLevelY(k) + tid.y, an offset counted twice
  (0038e824f). The copper block is copper (36.0 vs 32.9 reference display level). (4) That bug is
  why Polar's probe-only floors went dark once the probes stopped overshooting: their reflections
  came from black captures. Left floor 0.38 -> 0.69, Polar off 0.332 -> 0.225, on 0.191.
  Ten scenes after the pass, off (on): Polar 0.225 (0.191); panel room 0.251 (0.226); sunlit room
  0.311 (0.345); sky room 0.113 (0.170); courtyard 0.033 (0.037); sky-lit box 0.131 (0.139); glazed
  0.129 (0.103); unglazed 0.092 (0.076); open ground 10 deg 0.061 (0.092), 2 deg 0.087 (0.160). Mean
  0.143 off (0.127 before), 0.154 on: working captures now reflect rooms whose probe GI overshoots.
  (6) Rough-metal ceilings: every source overshoots (sunlit room probes 3.0x, captures 1.95x, base
  only 1.09x because captures were black). With one bounce (no probe feedback) the sunlit room's
  left floor already reads 1.05x and the rest 0.07-0.4x: the first bounce off the sunlit red wall
  onto the floor at its foot is about 2x strong (probes a few tenths of a metre out see the patch
  at better angles), multi-bounce feedback amplifies it, and the metal ceiling mirrors it. Footprint
  sun tests changed nothing there. Open: probe irradiance near bright sunlit walls. (7) Screen GI
  stays off (D32): worse than probes on open ground (+7-9%; part rough specular, whose GGX levels
  renormalise a grazing lobe's below-horizon share to the rest's mean; part diffuse), the sky room,
  the box and the sunlit room. Tried and reverted: misses from the fallback's irradiance (open
  ground better, sky room 0.17 -> 0.22: missed rays point at the window) and cone-filtered misses
  (no change).
  Accuracy pass (2026-11-01, branch t3/fix-probe-gi-overshoot, scratch `D:AraScratchgi3`):
  isolation rooms (the sunlit room with every other surface black or white and no sky; up-facing
  sensor cards; a mirror sphere; a top-down floor view against `--reference --bounces 2` and a CPU
  model of the room, `gi3/sim`) split the overshoot into three causes. (1) Relighting tested the
  sun once per 22-degree texel (a virtual page is a point compare; the cascade path's one disc PCF
  turns a patch edge all-or-nothing), so a sunlit patch a few texels wide aliased: probe columns
  alternated 0.6x and 1.8x on the floor past the sunlit wall. The sun now averages 2x2 footprint
  points, half a disc each (4x4 measured no better at three times the cost), glass tint per point
  (9c5d4391d, f83fb431c). The hit plane continues past its surface (a floor's beyond the walls, a
  wall's across its window), so the bake records which of a texel's 16 sub-rays lie on the relit
  plane (16-bit mask; emissive moves to RGB9E5; surface version 14) and footprints use those only.
  (2) A texel's albedo averages all its hits but its direct light was the relit plane's for all
  of them: a texel across a sunlit patch's edge credited the ceiling beside it, in shadow, with
  the patch's sun. One bounce in a white isolation room read 1.6-1.8x on the walls and floor, and
  compounding over bounces 2.4x. Off-plane hits now take the lesser of the texel's direct light
  and that of the neighbouring texel they lie towards (e43db8027); the neighbour's alone read
  Polar right but doubled the white room (a 22-degree neighbour footprint is not its edge strip).
  (3) Reflection capture faces kept the glow of emissive panels promoted to rect lights, which
  already reach every specular lobe: the panel room's metal ceiling 2.05x, back wall 1.45x (the
  task's open back wall item); faces now drop emission above the emitter threshold (883a542ba).
  Near-field interpolation is real but secondary: with exact probe values, the 0.29 m probe layer
  reads 1.2-2.3x within 0.6 m of a lit wall (CPU model). A per-probe log-irradiance gradient from
  the radiance cache's hits reprojected half a cell either side matched the model's slopes but
  made a hot spot and a general +5-15% in the shader; parked (`gi3/parked`). An energy-conserving
  bounce albedo ((1 - F) diffuse plus specular, as the path tracer) improved the sunlit room 0.142
  -> 0.109 and the box 0.131 -> 0.107 but darkened both glass rooms 8% (mean 0.101 -> 0.108);
  reverted, so the glass rooms hide another missing ~8%. Ten scenes, High, base 111eef61b ->
  now, screen GI off (on): Polar 0.224 -> 0.214 (0.191 -> 0.257); panel room 0.251 -> 0.135
  (0.228 -> 0.154); sunlit room 0.311 -> 0.098 (0.345 -> 0.157); sky room 0.113 (0.170);
  courtyard 0.033 (0.037); sky-lit box 0.131 (0.139); glazed 0.130 -> 0.049 (0.103 -> 0.053);
  unglazed 0.093 -> 0.054 (0.076 -> 0.054); open ground 10 deg 0.061 (0.092), 2 deg 0.087 (0.160).
  Mean 0.143 -> 0.098 off, 0.154 -> 0.127 on. Cost at 2560x1440 High (median of 5, `ara-dev gpu`,
  CPU guard tripped by idle desktop apps only): GI.Relight Polar 0.29 -> 0.40 ms, sunlit room 0.12
  -> 0.17 ms; GI.Gather unchanged (0.11 ms). Polar Epic flicker (`--stability 30`, 1080p): the tip
  read 7.6% of blocks (1.0% before a0b08334c's single-disc footprint sun), now 1.5%; High 0.3%,
  sunlit room 0%. Glass rooms on cascades alone read 11% dark (the last pass measured 15%) in both rooms, glazed or
  not: not the lift (a cascade-sized lift reads darker), not the kernel (0.75 texel the same); the
  maps themselves, camera-fitted cascades for hits around and behind the view, which pages cover.
  Open: Polar with screen GI (0.257; left floor 0.5x, 58% of it reflection), the near-field
  interpolation (gradient parked), rough-metal ceilings (sky room 1.31x, box 1.11x, panel 1.46x),
  `--without screen-reflections` flicker on Polar Epic 17% at 30 frames (tip 30.9%, 2.0% before
  0038e824f): reflection captures follow the probe volume's slow multi-bounce convergence through
  their refit ratio (0.2% after 300 frames; 16 initial bounces 13.6%), hidden while captures
  reflected black.
  Reflections pass (2026-11-01, branch t3/459259f4, scratch `D:AraScratchgi4` with sheets and tables):
  (1) Polar's smeared floor was not roughness: its corridor strip is 0.125 (alpha 0.016), and a copy
  of Polar with every metal at roughness 0.05 still smeared. `SSR.Resolve` averaged its neighbours'
  colours over the lobe's footprint on top of the trace's own cone filter, blurring every glossy
  reflection twice; it now keeps the hit's own colour (tent-filtered with its four same-surface
  neighbours) and spreads only confidence, the trace's pyramid level spans at least its five taps'
  spacing (a rough floor streaked a sphere instead of printing five copies of it), and the
  full-resolution upsample weighs normals (no stair-stepped strip edges) (0dd2d6068). What Polar's
  floor still lacks is different content, not blur: the reference reflects the ceiling skylights where
  Ara's capture traces meet the corridor's overhead frames, and Ara's door ring reads lit grey where
  the reference's is near black. (2) The `--stability` flicker while captures bake was not the refit
  ratio (frozen at one: 16.6%, unchanged) but the bake frames themselves: each face rendered through
  the full frame and relit the shared probe volume with the capture camera's cascades and no virtual
  shadow pages, so probes alternated between two sun estimates. Bake frames now only read the volume
  (`GiGpu::frozen`), and a capture's first bake fades in over 48 relights (at once for bakes of every
  step, restores and unseen frames) (ff0fa1a0a, 0a7410c95). Polar Epic `--without screen-reflections
  --stability 30` at 1080p: 17.4% -> 0.2% of blocks; Epic 1.1%, High 1.0%. (3) Rough-metal ceilings:
  the path tracer's GGX was single scattering while the raster compensates multiple scattering as
  Cycles' Principled v2 does; a roughness-0.7 metal seen at grazing angles lost up to half its
  reflection in the reference. `ara-pathtrace` now takes the same Kulla-Conty scale and fits
  (30ded3353), and every table below is against the new references (old ones kept as `*-ref0.png`):
  the sunlit room's ceiling moves from 1.18x to 0.67x (1.04x with screen GI), the sky room's from 1.31x
  to 0.93x (1.11x). With screen GI off the sunlit room's ceiling now reads low (0.67x): open. (4) The panel room's floor strip: SSR hits now drop
  emission promoted to emitter lights, as captures and screen probes do (b05b9dc61): 0.99x -> 0.92x off,
  1.09x -> 1.02x on. The reference shows no reflection of the panel in the strip at all, where Ara's
  rect-light specular draws one: open (the path tracer reaches emissive meshes only through light
  samples). (5) Not reworked: the SSR-to-probe step under the shapes scene's lamp pools. (6) Screen GI
  on open ground is not the rough lobe: reflections are about 1% of a roughness-1 ground's radiance; the
  +7-9% is diffuse. No 4x4 irradiance texel faces the probe normal, so pixels interpolate tilted ones
  whose below-horizon share takes the sky's mean. Filling it with the probe surface's own radiance or
  with nothing brings open ground 0.162 -> 0.143-0.151 but costs the sunlit room 0.079 -> 0.097;
  reverted. Solid-angle weights in the rough lobe: no change. (7) The energy-conserving bounce albedo
  ((1 - F) diffuse) measured again against the new references: sunlit room 0.142 -> 0.208, sky room
  0.124 -> 0.142, panel 0.120 -> 0.136, box 0.126 -> 0.103; reverted. (8) Near-field interpolation:
  not reworked. Ten scenes, High, base b50a7656a -> now, mean |ln ratio| against the multiple-
  scattering references, screen GI off (on): Polar 0.214 -> 0.192 (0.258 -> 0.240); panel room 0.132
  -> 0.123 (0.116 -> 0.095); sunlit room 0.155 -> 0.142 (0.080); sky room 0.124 (0.130); courtyard
  0.014 (0.015); sky-lit box 0.126 (0.137); glazed 0.049 -> 0.052 (0.053); unglazed 0.057 -> 0.060
  (0.055); open ground 10 deg 0.060 (0.091), 2 deg 0.086 (0.159). Mean 0.102 -> 0.098 off, 0.109 ->
  0.106 on (against the old references 0.098 -> 0.095, 0.127 -> 0.124). Cost at 2560x1440 High,
  median of 5 (`ara-dev gpu`, valid): Polar SSR.Trace 0.349 -> 0.358 ms, SSR.Resolve 0.115 -> 0.125
  ms, DeferredLighting 0.788 -> 0.799 ms; sunlit room +0.03 ms in all.
  Open: Polar's floor content (skylights the capture traces miss; the door ring's brightness) and its
  left floor (0.55x off); screen GI's tilted irradiance texels; the bounce albedo; probes' radiance near
  sunlit patches seen from rough-metal ceilings; the panel reflection the reference lacks; the shapes
  scene's SSR-to-probe step; near-field interpolation.
  Earlier open items: (a) Polar's probe-only floors and back room went darker (left floor 0.62 -> 0.38);
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
  `--without volumetric-fog` and editor toggles.
  Per-pixel local lights (d56248fc4, reference f08e4f920, cook f9647dc43): beams were
  froxel-blocky (stair-stepped cone edges, a blocky crown at a spot's apex, stepped corners
  under a rect's barn doors) and the path-traced reference's own fog was a 64x36 grid.
  `Fog.Lights` (look-fog-lights.slang) integrates each local light of a pixel's tile along
  the pixel's ray: analytic clip to range sphere, outer cone, front half-space and barn
  doors; equiangular sampling (Kulla and Fajardo 2012) on fixed angular bins (cost follows
  the angle a light fills; continuous across pixels); shadows through a continuous
  bilinear tent of the moment pyramid at each sample's footprint (shadowFog `smooth`);
  froxel transmittance; area lights' exact near field in fog (luminance x solid angle of
  the visible part). Deterministic midpoints, no jitter, no history. Half resolution with
  depth-aware upsampling; 24/16/12 bins (Epic/High/Medium); groups cull their tile's lights
  by frustum and farthest depth. The near volume composites through a cubic B-spline across
  froxels (no Mach-band steps on sun shafts and volume edges). The reference traces the
  same fog model per pixel (ara-render-fidelity `shaders/medium.slangh`: strata with sun
  shadow rays, one traced ambient ray, one equiangular sample per light with an area sample
  and shadow ray). Fog beams scene vs the per-pixel reference (EV100 2): the cube's shaft
  darkens the fog under it (luminance against the scene without the cube) by 7.2% in the
  reference, 6.3% at Epic and 5.8% at High (0% in the old reference, which could not
  resolve it); a rect's flat near-field slab and its barn-door edges match. Stability (`--stability 30`, Epic): fog beams worst 16x16 block
  2.99 -> 0.26, 0% flicker; Polar unchanged (1.8 -> 1.9% of blocks, elsewhere).
  Cost, 1920x1080, RTX 4070 SUPER, median of 5 captures under `ara-dev gpu` (GPU guard
  valid; CPU guard relaxed for another agent's busy core), all fog passes before -> after,
  of which Fog.Lights: fog beams Epic 0.88 -> 1.03 (0.29), High 0.35 -> 0.57 (0.24), Medium
  0.20 -> 0.39 (0.21); Polar (54 spots) Epic 1.02 -> 1.34 (0.50), High 0.47 -> 0.73 (0.37),
  Medium 0.31 -> 0.55 (0.34) ms; deferred lighting about +0.1 ms (B-spline taps; that pass
  is bimodal, 1.16 or 1.62 ms for the same base frame). Polar's coarse grid 0.16 -> 0.08 ms.
  Crisp edges at 1:1 (5d811c7f1, d7970282a, cook 4b5816fb2): half resolution stepped hard
  edges in 2-pixel blocks (a rect's barn-door diagonal, the spot's apex and cone edges).
  `Fog.Lights.Resolve` now writes the render-size image the composite reads: even pixels
  take their own texel, the rest blend neighbours by depth; pixels whose same-surface
  texels differ by more than 10% in luminance (above a floor near black), or that share no
  surface with any (thin occluders), go to an edge list that `Fog.Lights.Edges`
  integrates at full resolution by an indirect dispatch (each pixel's angular bins over
  eight lanes, summed in lane order). Refined: about 0.6% of pixels (fog beams Epic), 0.4%
  (Medium), 0.5% (Polar Epic). The spot beam test compares half against full resolution
  across the beam: worst 2/255 (197/255 without the edge pass). Extra cost (same guarded
  method): resolve 0.046-0.048 ms plus edges 0.037-0.040 ms (fog beams) and 0.084-0.095 ms
  (Polar) at Epic/High/Medium; lighting no longer upsamples (about -0.02 to -0.04 ms).
  Open: local fog volumes in the fidelity renderer; froxel leaking through thin occluders at
  grazing angles (as Unreal); virtual shadow pages for fog; a fog buffer view; the reference
  ignores light-path extinction to local lights and the sun's in-scatter (as raster) and
  lays height fog over the physical sky only to the volumetric distance (as raster);
  faint horizontal bands in fog ambient from probe GI's trilinear lattice (pre-existing, ~2x
  contrast to see); a sawtooth at the foot of the back wall under the rect light in the fog
  beams scene is surface soft-shadow shading (pre-existing, fog off too). Polar's authored
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
  hand over to screen probes (D32). SSR.Trace 0.60 ms at 2560x1440 High. SSR.Resolve (04becdb8b)
  spreads each pixel's hit confidence over its GGX footprint, so rough floors fade between traced
  and fallback reflections instead of outlining what the mirror rays reach; its colour stays the hit's
  own cone (0dd2d6068: averaging it again blurred every glossy reflection twice), and hits drop
  emission promoted to emitter lights (b05b9dc61) (0.116 ms Polar High;
  a faint step stays visible at 3x contrast where SSR and the probe fallback differ: on the
  area-light shapes floor SSR reads within 7% of the path tracer, the probe fallback 1.10-1.24x
  under the lamp pools, so the step is the probes' reflected radiance of the lamp-lit wall, open). Open: per-pixel cone
  footprints from the Hi-Z cell, measured before/after cost at equal settings.
  Polar's "smeared" floor, root cause (2026-11-01, scratch `D:\AraScratch\polarrefl`): the reference's
  blue streaks are sky seen through the narrow slots between the hanging pipe bundle and the
  ceiling grilles, as the floor sees them (a floorless mirror-camera reference, flipped, shows
  them; bounces 1 has none). Ara's scene and lighting have them too, but the main camera and the
  eye-height captures see the pipes in front of the slots: SSR accepts the grazing rays as pipe
  hits (thickness 2% of depth, 20 cm at 10 m) and reflects pipe brown. Not normal maps (removed:
  stripes stay), not roughness, not capture exposure. Fix in progress, not merged: automatic
  planar reflections (G-buffer height histogram picks the floor, half-res mirror view clipped
  above it via SV_ClipDistance, atlas read by SSR.Trace) on WIP branch `t3/a12c4f62-planar-wip`
  (1737c32d2): builds, never rendered, shaders uncooked, adds a Vulkan shaderClipDistance
  requirement. The hybrid RT path also misses the stripes (its misses take probe light, unchecked).
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

- [x] **Capture exposure defaults.** A scene with no post-process volume captured at the
  legacy manual exposure 0.0015 (EV 9.1, daylight), so lamp-lit scenes came out ~4.5 stops
  dark unless `--ev100` was passed, and the capture JSON reported `"exposure": 0.0015` even
  under `--ev100`. Now `World::capture` gives a volume-less scene one empty global volume
  (`SceneLook::meter_without_volumes`), so it meters like the volume defaults do; the
  settle frames snap the meter, so captures stay deterministic (two runs, same PNG hash).
  The JSON's `exposure` is the linear multiplier applied (`Presenter::applied_exposure`):
  the `--ev100` lock (2 gives 0.2083), else the metered or manual value. The area-light
  shapes scene went from near black (0.0015) to metered 0.0923 (EV 3.4). `--ev100` still
  locks over everything; live Play and the editor keep the legacy exposure for scenes
  without a volume (only captures changed). Scenes that already had a volume are unchanged.

- [x] **Stale player under `ara capture`.** `ara capture` renders through the sibling
  `ara-player` binary, and building only `-p ara` left an older player that loaded the
  tree's new cooked shaders against its old data (an old CPU probe bake drew star patterns
  over every wall); a missing or stale `ara-import-worker` left imported models as
  placeholders without a diagnostic (the glass rooms' windows went dark purple and blocked
  the sun). `ara-dev`'s editor flow guards this by building the companions in one Cargo
  invocation, which cannot cover `cargo build -p ara` run by hand, so the `ara` binary now
  checks at spawn (`ara-companion`, `ARA7308`): in a source checkout it refuses a player or
  import worker that is missing, or older than a source in its path-dependency closure,
  naming the binary, the changed file and the `cargo build -p ...` that fixes it. Packaged
  builds have no tree and are never refused; `ARA_ALLOW_STALE_COMPANIONS=1`, or an
  `ARA_PLAYER`/`ARA_IMPORT_WORKER` outside the `ara` directory, runs an old companion on
  purpose (before/after timing). Limits: an mtime rule (Cargo's own), so a stray `.rs` file
  Cargo never compiles can read as newer; `optional` dependencies (feature-only crates such
  as `ara-pathtrace`) are not followed.

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
