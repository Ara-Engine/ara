# ara-cmd

The command, query and job registry (decision A4). Every authoring mutation is a
typed command whose `run` changes documents only through `EditCtx`, so each change
becomes an op in a transaction (undo, review and the op stream for free).
Depends on ara-ops, ara-doc, ara-schema, ara-components, ara-diag.

## Public API

- Traits: `Command`, `Query`, `Job`. Each has `Params: Schema + DeserializeOwned`,
  `Output: Serialize`, `NAME`, `SUMMARY`, `SCOPE` (default `edit`, queries `read`),
  `HINTS` (`idempotent`, `destructive`), `examples()` and `run(ctx, params)`.
- `Registry::builtin(&mut db)`; `reg.command::<C>(&mut db)`, `.query::<Q>()`,
  `.job::<J>()` (registers the params type too); `info(name)`, `infos()`,
  `catalog()` (the `commands` section of reflection.json), `check_examples(&db)`.
- `reg.apply(&mut store, &Batch, &Session) -> Result<BatchResult, Vec<Diagnostic>>`;
  `reg.run(..., progress)` also reports job progress.
- `Batch { doc, calls: Vec<Call { command, params }>, why, idempotencyKey, base,
  dryRun }`; `Batch::new(doc, calls)`.
- `Session { author, scopes }`, `Session::human()`; scopes are exact (`edit`),
  prefixed (`edit*`) or `*`.
- `BatchResult { dryRun, txn, replayed, revision, outputs, symbols, ops }`.
- Contexts: `ReadCtx` (`schema`, `workspace`, `doc`, `target`, `resolved`, `entity`,
  `has_component`, `order_at`, `check_value`), `EditCtx` (`read`, `new_id`,
  `create`, `set`, `delete`, `move_to`, `set_entity`), `JobCtx` (`edit`, `progress`).
- `Target::{Plain(id), Prefab { instance, inner }}`: an addressed entity.

## Starter set (`builtin`)

`entity.create`, `entity.delete`, `entity.reparent`, `component.add`,
`component.remove`, `property.set`, `transform.set`, `prefab.instantiate`,
`prefab.override.set`, `prefab.override.revert`; query `entity.inspect`.

Edits to a prefab instance, or to an entity inside one (`e_4f9a/e_lid1`), become
overrides on the instance record; deleting an inner entity adds it to `removed` and
drops overrides aimed at it.

## Batch semantics

- One batch is one transaction on one document. All calls run in order against the
  live staged state; any error rolls everything back.
- Params are validated against their schema before `run` (diagnostic paths start
  with `params.`), then deserialized with serde.
- Symbols: a create call with `"id": "$name"` mints an ID and binds it; later calls
  can use `$name` anywhere a string is expected, also as an address head
  (`$crate/e_lid1`). `symbols` in the result maps them.
- `dryRun` runs everything, runs the commit checks, rolls back and returns the same
  ops, outputs and symbols an apply would produce (the ID generator is restored).
- Errors carry `location.command` (the call index).

## Writing a command

- Params struct: `#[derive(Schema, Deserialize)]`, `#[ara(params)]`,
  `#[serde(rename_all = "camelCase", deny_unknown_fields)]`; doc every field; units
  on physical fields; `Option` for optional params.
- Validate early and return a `Diagnostic` with a stable code; prefer `ReadCtx`
  helpers (`target`, `check_value`, `order_at`) over re-implementing lookups.
- Never mutate documents except through `EditCtx`.
- Give at least one example; `check_examples` (tested) validates and deserializes it.

## Testing

`cargo test -p ara-cmd`: examples and catalog (snapshot), a scripted batch using
every starter command (dry run equals apply; resulting scene snapshot; undo restores),
deletes inside instances, located errors, scopes, and a job with progress.
