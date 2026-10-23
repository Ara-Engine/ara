# ara-graph

Shared authoring graph documents, deliberately independent of execution domains.
No editor UI, GPU, ECS or file IO. Depends on the kernel and BLAKE3.

## API

- `register(&mut SchemaDb)`: registers graph records and `graph` document kind.
- `Graph`: stable `n_` node / `w_` edge IDs; ports have persistent keys scoped to
  their node, so an endpoint is `(node, port)` rather than an array index.
- `Node`: registry kind/version, typed ports, parameters, opaque payload and an
  optional full `@path#id` subgraph reference. Unknown payloads are never discarded.
- `NodeRegistry`: documented exact-version metadata and stable-order search.
- `DomainAdapter`: compatibility, cycle policy, node checks and compilation.
  Generic authoring permits feedback; the adapter decides whether to compile it.
- `Graph::{to_document,from_document,read,write}`: canonical kernel storage;
  schema ordering/default omission and all JSON writing belong to ara-json.
- `Graph::{validate,check_wiring,topological_order,semantic_hash}`: headless checks,
  deterministic ordering and BLAKE3 compilation identity.
- `commands::register`: add/remove node, connect/disconnect, set parameter, move,
  and edit canvas comments/groups/reroutes. All writes use ara-cmd EditCtx/ara-ops.
- `commands::apply_snapshot`: domain projections reconcile snapshots into kernel
  field ops, preserving revision conflicts, attribution, dry-run and undo.

## Storage and gotchas

- `g_meta` owns semantic domain metadata; `l_editor` owns only canvas data.
  Node positions, comments, groups and edge reroute points are excluded wholesale
  from semantic hashing. Reroutes are visual control points, not execution nodes.
- Nodes/edges are individual records, not a JSON blob on an entity. Moves address
  `l_editor.positions[n_id]`; graph data and canvas edits can be undone separately.
- JSON equality uses ara-json numeric equality (`1` equals `1.0`) for opaque data.
- Schema names carry Graph prefixes because the Schema DB namespace is global.
- Snapshot construction is for initial/imported documents. Never replace a live
  Store's document to implement authoring; submit a command or op transaction.
- Generic commands check structural wiring but cannot enforce plugin compilation
  rules. Call the adapter before publishing compiled artifacts. Invalid/unknown
  graphs stay inspectable, with their original payloads available for repair.
- Diagnostic codes 3100-3103 are local pending central catalog integration.

## Testing

Set `CARGO_BUILD_JOBS=2` while other production tracks build.
`cargo check -p ara-graph`
`cargo test -p ara-graph`
`cargo clippy -p ara-graph --all-targets -- -D warnings`
`cargo fmt -p ara-graph -- --check`

Tests cover unknown payload preservation, canonical JSON, layout hash exclusion,
registry/domain/cycle rules, deterministic order, atomic command rollback and
kernel undo/redo. Do not add UI tests here; this crate does not draw the canvas.

## Renderer adoption

ara-render consumes the material adapter in ara-material, never raw graph nodes.
Use graph semantic hashes to avoid scheduling material cooking after canvas moves.
Keep compiled results outside the authoring Store and swap last-good results only
at frame boundaries under the human's application policy (D20).
