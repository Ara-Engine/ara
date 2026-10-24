# ara-components

Schemas and ECS types of the built-in engine components: `Transform` (position in m,
rotation as XYZ Euler degrees, scale), with replication policy, and `render`: the
authorable look (`SkyAtmosphere`, `HeightFog`, `PostProcess`, gathered as `SceneLook`)
and the project's `RenderSettings { quality, ray_tracing }` (`ara.project.json` → `rendering`).

## Public API

- `Transform { position, rotation, scale }` (`Default` is identity).
- Built-ins derive Ara `Schema`, Bevy 0.20 `Component` and Serde; omitted JSON
  fields materialize the same defaults as the schema. No runtime registry is required.
- `register(&mut SchemaDb)`.
- `render`: renderer-neutral data only; `ara-render-fidelity::environment` maps it
  onto both renderers and `presets` maps `QualitySetting`.

## Rules

- Subsystem crates will own their components (render, physics, audio) as they land.
  Put a component here only if the kernel itself needs it.
- Every field needs a doc comment, a unit when it is physical, and a replication
  policy when it is gameplay state.
- Keep any Rust `Default` in sync with the schema defaults (a test checks `scale`).

## Testing

`cargo test -p ara-components`.
