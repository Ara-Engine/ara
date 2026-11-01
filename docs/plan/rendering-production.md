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
  path-traced shapes, fog through look-lights.slangh). Open: penumbrae capped at 4 texels (wide
  soft shadows need filterable shadow maps), area-light soft shadows in fog, rect source
  texture, emissive surfaces promoted to area lights. Original item: Rect, disc, sphere and tube lights (`LightKind`), LTC specular and
  diffuse (Heitz 2016), barn doors for rects, optional light texture (rect), IES / light
  profiles for point and spot ("shaped lights"), source radius/length softening, their
  shadows (local faces, contact-hardened by source size), GI relight and fog. Unreal:
  `URectLightComponent`, source radius/length, `IESTexture`.
  Done (2026-11-01): `Light` kinds Rect/Disc, `LightUnits` (lm/cd/nits), source radius/length,
  rect size and barn doors, `sourceAngle`; 112-byte record; LTC shading fitted to Ara's GGX
  (`examples/ltc_fit.rs`, exact per-edge horizon clipping, ellipse form factor for discs and
  spheres) in deferred, forward, ray hits, probe relight and fog; IES import (`ara.ies`) with
  profiles in raster and the path tracer; reference samples every shape with MIS hits; PCSS
  local shadows; light complexity view; editor gizmos and Create entries. Open: penumbrae
  wider than 4 shadow texels (big rects stay hard-edged; needs filterable shadow mips) and the
  hard self-shadow terminator of centre-point shadow maps; rect source textures; promoting
  large emissive surfaces to rect/disc lights; Unreal converter rect export.
- [ ] **Emissive surfaces light the scene.** Emissive meshes feed probe GI (hit radiance)
  and screen-space GI; large or bright emitters can be promoted to area lights for direct
  light and shadows (Lumen's emissive lighting; Unreal's "Emissive Light Source").
- [ ] **Soft sun shadows.** Source-angle penumbrae on cascades and the far map (PCSS
  blocker search, the VSM path already contact-hardens), matching the light's angular
  diameter (`Light.sourceAngle`, Unreal's `LightSourceAngle`). `sourceAngle` is authored and
  feeds `Look.sunLux.w` (VSM PCSS) and the path tracer's sun cone (2026-11-01); cascades
  and the far map are open.
- [~] **Global illumination ("Lumen alternative").** Screen probe GI (D32, `look::screen_gi`,
  High/Epic, `--without screen-gi`, editor View > Rendering Debug > Screen GI, yellow in
  `--reflections sources`): 16 px probes on the G-buffer trace 64 fixed directions through a
  Hi-Z pyramid; hits from last frame's colour (minus promoted emission, capped at what a diffuse
  surface of the hit's albedo sends, as the bitmask AO's bounce), misses from the probe volume;
  3x3 tent gather; rough lobes by the view's true GGX lobe. Deterministic, no history.
  Measured (High, EV-locked, 1280x720, mean |log ratio| of region luminance vs the path-traced
  reference, `D:AraScratchgi`, before -> after): Polar 0.525 (old SSR) / 0.554 (Hi-Z SSR) ->
  0.468 (ceiling metal 0.57 -> 0.63, side floors 0.42/0.48 -> 0.49/0.50; glossy floor centre
  0.55 -> 0.52 as Hi-Z SSR now hits the too-dark ceiling); sky-lit courtyard 0.049 -> 0.052
  (metal shed 1.40 -> 1.31); sunlit room 0.253 -> 0.382 (ceiling 1.41 -> 3.7); room with an
  emissive panel 0.127 -> 0.302 (whole frame 1.13 -> 1.45, ceiling 1.27 -> 2.71; 3.10 before the
  hit cap). Both rooms regress on their rough-metal ceiling: with captures off the old path
  shows the same overshoot (3.35x), so it is the probe volume's, which High's darker captures
  had masked. Cost at
  2560x1440 High: ScreenGI.Probes 0.69 ms + Gather 0.28 ms (+0.94 ms frame); stability equal
  (0.2% vs 0.1% blocks). Findings: (1) the probe volume overshoots light entering through
  openings: sky through a window lands 3.4x too bright on a room's back wall even with
  single-bounce relight, and the volume's rough specular is 3.35x on a metal ceiling (the main
  open error; screen probe misses inherit it); a thicker ceiling and a smaller sun-test lift
  change little, so it is not slab leakage; check per-texel solid angles in the octahedral
  convolution and the sky-fraction texels next. (2) Reflection captures as the miss fallback are
  too dark (Polar ceiling 0.64 -> 0.33; a black capture in the test room): rejected. (3) Emissive
  fixtures promoted to emitter lights (spot clusters) make a hotspot under a panel that GI then
  bounces (panel room 2.85x): promote large panels to rect lights (Area lights item). (4) Polar's
  ceiling gratings read opaque where the reference sees sky through them (cutout transparency,
  not GI; the "ceil metal R" region is 89% direct). Open: the probe-volume overshoot, adaptive
  extra probes where the 3x3 gather finds none, Epic's quality (tile 8 cost 3.4 ms), an
  in-engine region tool, Medium screen GI.
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
