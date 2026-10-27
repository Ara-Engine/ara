# ara-doc

Authoring documents (scenes, prefabs) as flat maps of records keyed by short IDs,
their canonical files, `@path#id` references, prefab instances and the op
vocabulary. Depends on ara-schema, ara-json, ara-diag. No ECS, no IO.

## File format

```json
{
  "$schema": "ara://schema/scene/1",
  "id": "sc_arena001",
  "versions": { "Health": 2 },
  "records": {
    "e_lmp1": { "name": "Lamp", "parent": "e_stk2/e_c001", "order": "a0",
                "Light": { "kind": "spot" }, "Transform": { "position": [0, 1, 0] } },
    "e_stk2": { "prefab": "@prefabs/stack.prefab#pf_stack001",
                "overrides": { "e_c001/e_lid1/MeshRenderer.tint": [0.2, 0.9, 0.2, 1] },
                "removed": ["e_c002/e_lid1"] }
  }
}
```

- Record IDs `<prefix>_<base32>` (prefix picks the record type: `e` = `Entity`);
  asset IDs `sc_`/`pf_` + 8 chars, unique per project.
- Built-in fields (lowercase) in reflection order, then components (PascalCase),
  sorted. Defaults omitted. `versions` lists types newer than version 1.
- `parent` is an entity address; `order` is a fractional index (`order.rs`).
- Instances carry `prefab`, `overrides` (`[<id path>/]<field path>`) and
  `removed`, never inline components. Only `name` and components are overridable.

## Public API

- `read(text, file, &db, Option<&AssetIndex>) -> ReadResult { doc, diags }`;
  `read_header` (kind + ID only, for indexing).
- `write(&doc, &db) -> String`, `to_value`, `canonical_record`.
- `Document { kind, id, conflicts }` + `records()`, `record()`, `children()`,
  `subtree()`; helpers `parent_of`, `order_of`, `prefab_of`, `is_component_key`.
- `Edit` → `doc.apply(edit, &db) -> Result<Op, Diagnostic>`; `Op::inverse()`,
  `Op::to_edit()`, `Op::touches()`; `doc.revert(&op, &db)` (rollback).
- `Workspace::apply` / `revert` handle ordinary edits plus `Edit::Document`
  lifecycle ops. Create/remove empty documents inside the transaction; paths must
  be canonical relative paths. Windows document paths also reject case aliases.
- `refs::{AssetIndex, resolve, repair_record, record_ref}` (`AssetIndex::imports`:
  imported media `(id, path)`, what a copy of the documents needs to resolve
  references; `AssetIndex::closest`: the
  nearest assets of a kind to a broken reference; ARA1302 carries a `set` fix per tied
  candidate; `@path#record` into a kind with addressable records is kept as written,
  the record's existence is the consumer's check); `prefab::{resolve, resolve_record
  (one record's part of `resolve`, at its own cost; `resolve_record_reporting` also
  reports its problems), check_parents (the one check across records; with every
  record resolved alone it reports exactly what `resolve` does), check_instance,
  DocLookup, ResolvedEntity}`; `validate::{check_record, check_structure, check_entity_refs,
  record_type}` (dangling `entity` fields are warnings, `ARA1212`).
- `Workspace`: loaded documents + index; `find(id|path|@ref)`, `resolve`, `check`,
  `check_local` (what given records report on their own; local edits, see
  `Edit::is_local`, change nothing else), `remove` (a deleted file). `AssetIndex::remove`.
  `generation()` changes whenever a document is loaded, replaced or unloaded (never on
  record edits): caches of anything read across documents compare it.
- `Workspace::insert_with_asset_path(save_path, doc, source_path)`: load a sidecar
  whose asset references name source media; `path_of` remains the save path.
  Existing `insert(path, doc)` uses `path` for both purposes.
- `IdGen` (seedable SplitMix64): `record_id`, `asset_id`, `token`.
- `file_state::FileState` / `FileKind`: immutable SHA-256 tokens for files and empty
  directories. `Edit::File` / `Op::File` change the workspace's observed inventory,
  never the disk; host adapters materialize them. See `docs/dev/file-history.md`.
- `OrderKey`, `key_between(a, b)`.
- `register(&mut SchemaDb)`: the `Entity` record type and `scene`/`prefab` kinds.

## Rules and gotchas

- Mutate documents only through `Workspace::apply` from an ara-ops transaction.
  `insert_unchecked` is for readers and merges.
- `apply` checks only what holds regardless of the other ops in the transaction
  (existence, path, value type) and leaves the document unchanged on error.
  Parents, loops and record validity are checked at commit
  (`Workspace::check`). This keeps every op's inverse applicable.
- Reference repair needs an `AssetIndex`; without one, references are only
  checked syntactically. The ID wins when the halves disagree.
- Overrides apply in sorted key order (whole component before its fields);
  `removed` applies first.
- `$conflicts` entries read as ARA1209 with `ara resolve <file> --pick
  <record>:<path>=ours|theirs` fixes (`Cli` actions); the file needs a location.
- Problems inside a prefab are reported when that prefab is checked, not by every
  scene that instantiates it.
- Diagnostics follow file order of records; resolution output is sorted by address.

## Testing

`cargo test -p ara-doc`. Fixture project: `tests/fixtures/` (types, two prefabs
with nesting, one scene, one messy input). `tests/docs.rs`: byte-identical round
trip, normalization and repair (snapshot), syntax positions, prefab resolution
(snapshot), override/removal errors, structure errors, dangling entity references. `tests/inverse.rs`: property
test that apply → undo (inverse edits, and rollback) restores the document.
