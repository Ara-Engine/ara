# ara-components

Schemas and ECS types of the built-in engine components: `Transform` (position in m,
rotation as XYZ Euler degrees, scale), with replication policy; `render`: the
authorable look (`SkyAtmosphere`, `HeightFog`, gathered with the volumes as `SceneLook`)
and the project's `RenderSettings { quality, ray_tracing }` (`ara.project.json` →
`rendering`); and `post`: post-process volumes.

## Public API

- `Transform { position, rotation, scale }` (`Default` is identity).
- Built-ins derive Ara `Schema`, Bevy 0.20 `Component` and Serde; omitted JSON
  fields materialize the same defaults as the schema. No runtime registry is required.
- `register(&mut SchemaDb)`.
- `render`: renderer-neutral data only; `ara-render-fidelity::environment` maps it
  onto both renderers and `presets` maps `QualitySetting`.
- `post::PostProcessVolume { shape (global/box/sphere from the entity pose),
  blend_distance, priority, weight, overrides: list<PostOverride> }`. A `PostOverride`
  is a `module` enum plus `option<T>` properties shown per module (`showIf`); `None`
  means "not overridden". `SceneLook::post_at(camera)` blends every extracted
  `PostVolume` into `PostValues` (Unity's stack: ascending priority, ties in ID order;
  numbers lerp by the weight at the camera, enums switch at any weight; only the
  entry's own module applies). Preset-relative values (AO radius, SSR distance) are
  `PresetBlend { keep, add }` until the renderer knows the preset.
- `PostProcessVolume` is version 2 with alias `PostProcess`: old scenes upgrade on
  load into a global volume overriding every former field (defaults included).

## Rules

- Subsystem crates will own their components (render, physics, audio) as they land.
  Put a component here only if the kernel itself needs it.
- Every field needs a doc comment, a unit when it is physical, and a replication
  policy when it is gameplay state.
- Keep any Rust `Default` in sync with the schema defaults (a test checks `scale`).

## Testing

`cargo test -p ara-components`.
