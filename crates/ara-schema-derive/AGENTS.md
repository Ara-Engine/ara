# ara-schema-derive

`#[derive(Schema)]`: turns Rust structs and unit enums into `ara_schema::TypeDesc`s.
Re-exported as `ara_schema::Schema`; depend on ara-schema, not on this crate.

## What it generates

- `impl ara_schema::Schema` (`describe()`, `register_deps()`).
- `impl ara_schema::FieldType` (the type can be used as a field; registering a type
  registers its field types too).

## Rules it enforces at compile time

- Doc comments on the type, every field and every variant (they become tooltips,
  docs and agent tool descriptions).
- Named-field structs or unit-only enums; no generics.
- Defaults and examples are JSON-like literals (numbers, strings, bools, `null`,
  arrays). Objects aren't supported: give nested structs their own defaults.
- Typed attributes: `unit = Meters`, `audience = Relevant`, `interpolate = Slerp`
  are enum variants, so typos are compile errors.

Attribute reference: crate docs in `src/lib.rs`.

## Naming

Fields are stored camelCase (`max_speed` → `maxSpeed`) and variants with a
lowercase first letter (`SpotLight` → `spotLight`), matching
`#[serde(rename_all = "camelCase")]`. Types that are also deserialized with serde
(command params) must use that serde attribute; ara-cmd tests check every command's
examples deserialize.

Params role: fields that are not `Option` and have no `default` are `required`.

## Testing

Exercised through `cargo test -p ara-schema` (`tests/schema.rs` snapshots the
reflection output of derived types).
