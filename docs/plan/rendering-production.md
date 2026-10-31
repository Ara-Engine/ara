# Production rendering program

Goal: Ara's raster renderer at or past Unreal 5's lighting quality, at a lower frame cost,
for any project. The Polar facility comparison (`D:\AraScratch\UECompare` against
`AraPolar`) is a yardstick, never the target: every item is general engine work with its
edge cases, tests, debug views and CLI access. Ray tracing stays an opt-in (D27, D29).

Each item lands as its own commit(s), with focused tests, a debug view where the effect can
be isolated, and a `ara capture` switch where that helps diagnosis. Status: `[ ]` open,
`[~]` in progress, `[x]` done (with the commit).

## Lighting

- [ ] **Area lights.** Rect, disc, sphere and tube lights (`LightKind`), LTC specular and
  diffuse (Heitz 2016), barn doors for rects, optional light texture (rect), IES / light
  profiles for point and spot ("shaped lights"), source radius/length softening, their
  shadows (local faces, contact-hardened by source size), GI relight and fog. Unreal:
  `URectLightComponent`, source radius/length, `IESTexture`.
- [ ] **Emissive surfaces light the scene.** Emissive meshes feed probe GI (hit radiance)
  and screen-space GI; large or bright emitters can be promoted to area lights for direct
  light and shadows (Lumen's emissive lighting; Unreal's "Emissive Light Source").
- [ ] **Soft sun shadows.** Source-angle penumbrae on cascades and the far map (PCSS
  blocker search, the VSM path already contact-hardens), matching the light's angular
  diameter (`Light.sourceAngle`, Unreal's `LightSourceAngle`).
- [ ] **Global illumination ("Lumen alternative").** Bounce energy that matches the path
  tracer: screen-space GI over the probe field, a per-probe radiance cache with sky and
  emissive, multi-bounce, metals; temporal stability with no noise (D27). Measured against
  `ara-pathtrace` on the samples and Polar.
- [ ] **Volumetric fog and light shafts.** Froxel volumetric fog (Unreal's volumetric fog):
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

- [ ] **C# graphics settings.** Quality presets and individual settings readable and
  writable from game scripts (Unreal's `UGameUserSettings` scalability).
- [~] **Debugging.** A debug view for every effect above (`ViewMode`, `RenderDebug`),
  `ara capture` switches for each (`--view`, `--without`), a path-traced reference capture
  (`ara capture --reference`), and region luminance/compare tooling. Done: `--view` for
  every view mode, vertex-colour and decal-coverage views (7d53e77ee).
- [x] Per-mesh Cast Shadows (3f2a35625); horizon shadows past the maps' reach (e3f974ddd).
