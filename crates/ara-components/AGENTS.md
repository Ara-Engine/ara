# ara-components

Schemas of the built-in engine components. Today: `Transform` (position in m,
rotation as XYZ Euler degrees, scale), with replication policy.

## Public API

- `Transform { position, rotation, scale }` (`Default` is identity).
- `register(&mut SchemaDb)`.

## Rules

- Subsystem crates will own their components (render, physics, audio) as they land.
  Put a component here only if the kernel itself needs it.
- Every field needs a doc comment, a unit when it is physical, and a replication
  policy when it is gameplay state.
- Keep any Rust `Default` in sync with the schema defaults (a test checks `scale`).

## Testing

`cargo test -p ara-components`.
