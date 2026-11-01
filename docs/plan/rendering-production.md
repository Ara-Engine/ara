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
- [ ] **Global illumination ("Lumen alternative").** Finding (2026-10-31, Polar at EV100 -2,
  4096-spp reference): raster matches the reference on walls, the opening and the back room
  (within ~10%); the metal ceiling is 2.4x short (0.0021 vs 0.0050) and the floor centre 1.6x
  (it reflects the ceiling). Ceiling = 0.0011 reflections (SSR-resolved, green in sources)
  + 0.0011 diffuse. With each reflection source off (`--without`): no screen reflections
  ceiling 0.0033, floor centre 0.0028 (= reference), right wall 0.0045 (ref 0.0024); no
  captures ceiling 0.0030. Screen traces and captures both return darker rough reflections
  than the probes, while the probes overshoot on the right wall: per-source bias to fix.
  Tried and reverted: tracing SSR along the GGX dominant direction (floor centre 0.0018 to
  0.0014). Likely cause: SSR reads last frame, so the ceiling and floor reflect each other
  already too dark (a dim fixed point); the missing energy is first-order bounce from the
  sunlit floor/snow into rough metal.
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
- [ ] **Atmosphere parity.** Hillaire sky atmosphere (transmittance, multi-scattering,
  sky view, aerial perspective LUTs) matching Unreal's SkyAtmosphere component values.

## Surfaces

- [ ] **Consistent glass.** One transmission model across raster, hybrid and path tracer:
  thin and solid glass, roughness, tint by thickness, shadows by transmittance, reflections
  of and through glass, sorting.
- [ ] **Screen-space reflections.** Hi-Z traced SSR (min-depth pyramid), stochastic rough
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
