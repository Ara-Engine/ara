# ara-cmd

The command, query and job registry (decision A4). Every authoring mutation is a
typed command whose `run` changes documents only through `EditCtx`, so each change
becomes an op in a transaction (undo, review and the op stream for free).
Depends on ara-ops, ara-doc, ara-schema, ara-components, ara-diag.

## Public API

- Traits: `Command`, `Query`, `Job`. Each has `Params: Schema + DeserializeOwned`,
  `Output: Serialize`, `NAME`, `SUMMARY`, `SCOPE`,
  `HINTS` (`idempotent`, `destructive`), `examples()` and `run(ctx, params)`.
  `SCOPE` defaults to `edit:scene` (commands, jobs) and `read` (queries).
- `Registry::builtin(&mut db)`; `reg.command::<C>(&mut db)`, `.query::<Q>()`,
  `.job::<J>()` (registers the params type too); `info(name)`, `infos()`,
  `catalog()` (the `commands` section of reflection.json), `check_examples(&db)`.
- `document_template(kind, factory)` initializes a format's canonical skeleton
  through staged edits; `EditCtx::create_from_template` dispatches it. Factories
  call `create_document` for the lifecycle and `apply_edit` for initial records.
- `reg.apply(&mut store, &Batch, &Session) -> Result<BatchResult, Vec<Diagnostic>>`;
  `reg.run(..., progress)` also reports job progress.
- `Batch { doc, calls: Vec<Call { command, params }>, why, idempotencyKey, base,
  dryRun }`; `Batch::new(doc, calls)`.
- `Session { author, scopes }`, `Session::human()`; scopes are exact (`edit:scene`),
  prefixed (`edit*`) or `*`. The vocabulary lives in `ara-proto` (`Scope`).
- `BatchResult { dryRun, txn, replayed, revision, outputs, symbols, ops }`.
- Contexts: `ReadCtx` (`schema`, `workspace`, `doc`, `target`, `resolved`, `entity`,
  `has_component`, `order_at`, `check_value`), `EditCtx` (`read`, `new_id`,
  `create`, `set`, `delete`, `move_to`, `set_entity`, `create_document`,
  `apply_edit`), `JobCtx` (`edit`, `progress`).
- `Target::{Plain(id), Prefab { instance, inner }}`: an addressed entity.
- `select`: entity selectors. `Selector::parse` (address, `$symbol`, `name:A/B`,
  bare), `by_names`, `find`, `describe`, `resolve(db, params_type, params,
  entities)` (rewrites selectors in entity-typed params), `references` (every
  entity string of a call; the host infers documents from them).

## Starter set (`builtin`)

`entity.create`, `entity.delete`, `entity.reparent`, `component.add`,
`component.remove`, `property.set`, `transform.set`, `prefab.instantiate`,
`prefab.override.set`, `prefab.override.revert`; query `entity.inspect`.
`doc.create`, `doc.delete`, `project.setMain` and `transform.lookAt` share this registry too.
Document deletion records child-first deletes and a lifecycle op in one transaction;
anchor it to another document (normally ara.project.json), which stays loaded.

Edits to a prefab instance, or to an entity inside one (`e_4f9a/e_lid1`), become
overrides on the instance record; deleting an inner entity adds it to `removed` and
drops overrides aimed at it.

## Batch semantics

- One batch is one transaction anchored to a document; workspace commands may
  create or configure other documents. All calls run in order against the
  live staged state; any error rolls everything back.
- Params are validated against their schema before `run` (diagnostic paths start
  with `params.`), then deserialized with serde.
- Symbols: a create call with `"id": "$name"` mints an ID and binds it; later calls
  can use `$name` anywhere a string is expected, also as an address head
  (`$crate/e_lid1`). `symbols` in the result maps them.
- Name selectors in entity params (`name:Crate`, `name:Floor/Crate`, prefab
  contents included) resolve against the staged document before validation. One
  match → its address; several → ARA1609 with a fix per match; none → ARA1204
  with a did-you-mean. A bare name is ARA1102 with a `set` fix per matching ID.
- ARA1204 for an ID another loaded document has carries a fix setting `doc`.
- Asset params may skip the `@` (`prefabs/crate.prefab`, or a bare `pf_crate001`).
- `instance/<prefab root ID>` addresses the instance itself.
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
deletes inside instances, located errors, scopes, a job with progress, and name
selectors (paths, prefab contents, ambiguity, bare-name and cross-document fixes).
