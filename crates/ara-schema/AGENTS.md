# ara-schema

The Schema DB (decision A2): the one registry of reflected types that formats,
validation, the inspector, bindings, JSON Schema and agent tools derive from.
Independent of bevy_ecs; depends on ara-diag and ara-json.

## Concepts

- `TypeDesc { name, doc, module, version, aliases, deprecated, shape, upgrade }`
  - `Shape::Struct { role, fields }` with `StructRole::{Component, Record(RecordInfo),
    Params, Value, Resource}`, or `Shape::Enum { variants }` (first variant is the default).
- `FieldDesc { name, ty, doc, default, required, unit, range, examples, since,
  deprecated, aliases, replicate, editor }`.
- `ValueType`: `bool i32 i64 u8 u16 u32 u64 f32 f64 string vec2 vec3 vec4 quat color
  entity asset asset<kind> json`, named types, `option<T> list<T> keyed<T> map<T>`.
  Text form round-trips through `Display`/`FromStr`.
- `DocKindDesc`: a document kind (`scene`), its `$schema` URI
  (`ara://schema/scene/1`), ID prefix, extension and record types.

## Public API

- Registration: `db.register::<T>()` (derive), `db.add(TypeDesc)`,
  `db.add_json_types(&value)` (`*.types.json`), `db.add_kind(..)`, then
  `db.verify() -> Vec<Diagnostic>` (names, docs, field types, defaults, examples,
  prefixes; also reports registration clashes).
- Lookup: `get`, `lookup` (follows aliases), `types`, `components`, `kind`,
  `kind_for_schema(uri)`, `record_type(kind, prefix)`, `zero(ty)`, `field_default(f)`.
- Output: `reflection()` (`reflection.json`), `type_json(t)`,
  `type_json_schema(t)` / `doc_json_schema(kind)` (JSON Schema 2020-12).
- Data: `Validator` (values, structs, records, components → diagnostics),
  `canonical_value` / `canonical_struct` (reflection order, defaults omitted),
  `normalize_record` / `normalize_component` (aliases + upgrade hooks),
  `visit` (walk every typed value with its path).
- Paths: `FieldPath` (`Transform.position.x`, `overrides[m4tz/Light]`,
  `Inventory.slots[s_7c].count`), `db.resolve_path(record_type, &path)` →
  `Resolved { steps, ty, field }`, and `path::{get, get_effective, set}`.
- `grammar`: names, IDs, entity addresses, `@path#id` syntax.
- Layout: `db.struct_layout(name) -> StructLayout { size, align, fields: [FieldLayout] }`
  and `db.value_layout(ty)`: the canonical C layout of blittable structs (fields in
  reflection order, C alignment) that ECS columns and generated C# share. Not blittable:
  `string`, `asset`, `json`, `option`, `list`, `keyed`, `map`.
- Runtime types: `db.remove(name)` drops a type and its aliases (C# modules re-register
  on reload); `StructRole::Resource` / kind `resource` marks world singletons.

## Rules and gotchas

- Type names are PascalCase, field and variant names camelCase. No dots in names:
  paths use `.`; namespaces are metadata (`module`). Renames go through `aliases`.
- Components always have defaults (zero value unless `default` is given). Only
  params fields can be `required`.
- `version` + `upgrade: fn(from, &mut Value)` upgrade *components* stored with an
  older version (the document's `versions` table). Nested value structs evolve
  through field aliases or their component's hook.
- The `json` type (and `map<json>`) has no schema order; canonicalization sorts
  its keys.
- `path::set` is atomic: on error the record is unchanged. It materializes omitted
  intermediates from defaults but never creates a missing component.
- reflection.json `types` entries are exactly the `*.types.json` format; keep
  `reflect.rs` the single definition of both directions.
- `options` on enum-typed fields in reflection.json is derived output and is
  ignored on import.

## Testing

`cargo test -p ara-schema`. `tests/schema.rs` covers derive output (insta snapshot),
reflection → dynamic round trip, verification, validation diagnostics and fixes,
canonical form, JSON Schema (snapshot), path resolution, upgrades, canonical C
layouts, and replacing runtime types (resources). Accept
intended snapshot changes with `INSTA_UPDATE=always` and review the diff.
