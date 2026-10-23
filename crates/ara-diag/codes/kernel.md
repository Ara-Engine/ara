# Kernel: files, schema, documents, references, prefabs, transactions, commands, merge, protocol, projects (ARA1000–ARA1999)

## ARA1001 JSON_SYNTAX: Invalid JSON
owner: ara-doc, ara-host

The document, type file or import sidecar could not be parsed as strict JSON. No document or definitions can be loaded from that parse; the diagnostic gives the line and column.

Causes:
- A missing quote, comma or closing bracket, often from an interrupted edit.
- Comments or trailing commas in a format that accepts only strict JSON.

Fixes:
- Correct the syntax at the reported location, then run `ara check <file>`.
- Run `ara fmt <file>` after the JSON parses to write the canonical form.

## ARA1002 DUPLICATE_KEY: Duplicate JSON key
owner: ara-doc

A JSON object contains the same key more than once. The reader rejects the entire document instead of silently choosing one value.

Causes:
- An edit duplicated a component, field or record key in the same object.

Fixes:
- Combine the intended values under one key or remove the unwanted duplicate, then run `ara check <file>`.

## ARA1003 IO: File operation failed
owner: ara, ara-host, ara-script, ara-editor

A filesystem operation failed. The message identifies the operation and operating-system error; the requested read, write or host setup did not complete. A journal append failure does not itself undo an already committed in-memory edit.

Causes:
- The named file or project directory is missing or inaccessible.
- Permissions, a file lock or insufficient disk space prevented the operation.

Fixes:
- Restore access to the path named in the message and retry the original command.
- For project discovery, pass `ara --root <project-dir>`; for a failed journal write, preserve the running host and restore write access before closing it.

## ARA1101 UNKNOWN_FIELD: Unknown field
owner: ara-schema, ara-doc, ara-script

An object contains a key its schema does not define. Validation fails; a document may still be available for inspection, but `ara fmt` will not write a document with errors. Command or C# value validation rejects the affected input.

Causes:
- A misspelled field name or a field from another type.
- An unsupported top-level document key.

Fixes:
- Run `ara schema <Type>` or use `ara_schema` to find the correct field name.
- Use `ara check --fix <file>` to apply a sole suggested rename or top-level removal; review the suggested change first.

## ARA1102 TYPE_MISMATCH: Wrong value type
owner: ara-schema, ara-doc, ara-cmd, ara-script, ara-anim

A value has the wrong JSON shape for its schema, command parameter or animation document. The affected operation fails validation. A malformed document header prevents loading; field errors can leave a parsed document that is unsafe to write back.

Causes:
- A string where a number, boolean, object or array is expected.
- A vector with the wrong number of numeric elements, an invalid entity address or a non-integer value for an integer field.
- A non-object document, record, `types` map or command argument object.

Fixes:
- Check `ara schema <Type>` for the expected shape and supply the value in that form.
- Correct a loaded entity through `ara do property.set` with `entity`, `path` and `value`; correct malformed file structure and rerun `ara check <file>`.

## ARA1103 OUT_OF_RANGE: Number outside range
owner: ara-schema, ara-script

A numeric value does not fit its integer representation or the field's declared range. Vector components are checked individually. Validation or C# value encoding fails; the value is not clamped automatically.

Causes:
- A value exceeds the minimum or maximum shown in the message.
- A number cannot be represented by the declared integer type; C# encoding also rejects fractional integers.

Fixes:
- Read the field's units and range with `ara schema <Type>`, then supply a representable value.
- Use `ara do property.set` with the corrected value, or correct the file and run `ara check <file>`.

## ARA1104 UNKNOWN_VARIANT: Unknown enum variant
owner: ara-schema, ara-script

A string is not a registered variant of the field's enum. The value fails validation or encoding; the message lists valid choices where available.

Causes:
- A typo, incorrect case or a variant from another enum.
- An obsolete variant not covered by an alias.

Fixes:
- Choose a variant from `ara schema <Enum>` and retry the command or `property.set`.
- If the diagnostic offers one replacement, `ara check --fix <file>` can apply it to the document.

## ARA1105 MISSING_REQUIRED: Required field missing
owner: ara-schema, ara-doc, ara-host, ara-script

A required input is absent. The command, schema lookup or C# encoding cannot complete. A document without its required `records` object is not loaded.

Causes:
- A command omitted a required parameter.
- A document has no `records` member, or `schema.describe` has neither `name` nor `names`.

Fixes:
- Inspect the command or type with `ara_schema` and supply every required parameter before retrying `ara_apply`.
- Restore the document's `records` object, using `{}` only when the document is intentionally empty, then run `ara check <file>`.

## ARA1106 UNKNOWN_TYPE: Unknown reflected type
owner: ara-schema, ara-cmd, ara-host, ara-script, ara

The Schema DB has no usable type under the requested name. Component validation, path resolution, schema lookup or C# encoding cannot proceed for that type.

Causes:
- A misspelled or outdated component name.
- A project or script type was not registered, or a command's parameter type is missing.

Fixes:
- Use `ara schema` or `ara_schema` to find the registered spelling; use `ara check --fix <file>` when a document diagnostic offers a single rename.
- Correct and load the defining `*.types.json` or script module; restart `ara serve` after changing project type files.

## ARA1107 DEPRECATED: Deprecated schema member
owner: ara-schema

The value uses a deprecated component, field or enum variant. This is a warning: the member is still accepted unless another diagnostic reports an error. The message includes its deprecation guidance.

Causes:
- Existing data still uses a member whose schema marks it deprecated.

Fixes:
- Follow the replacement guidance in the message and inspect the current type with `ara schema <Type>`.
- Migrate loaded entity values through `ara_apply` commands such as `property.set`, `component.add` and `component.remove`, then run `ara check`.

## ARA1108 RENAMED: Alias renamed
owner: ara-schema

A recognized old component, field or enum variant name was replaced with its current name in the parsed data. This warning does not prevent loading. The file is unchanged until it is written.

Causes:
- Stored data uses an alias declared by the current schema.

Fixes:
- Nothing further needs changing in memory; run `ara fmt <file>` to persist the current names once other errors are resolved.
- Update future `ara_apply` requests to use the names shown by `ara_schema`.

## ARA1109 INVALID_TYPE_DEF: Invalid type definition
owner: ara-schema

A reflected type or document-kind definition violates schema rules. An unparseable data-only type is skipped; registration verification reports invalid definitions before they can be relied on for validation and tooling.

Causes:
- Missing documentation, invalid names, duplicate field or variant names, or an invalid record prefix.
- An unknown field type, invalid default or example, inconsistent numeric range, or keyed item type without `id: string`.
- A malformed `*.types.json` definition or an unsupported definition kind.

Fixes:
- Correct the named definition in Rust reflection metadata or its `*.types.json` file according to the message.
- Run `ara schema` to verify project definitions, then restart `ara serve` to load corrected project types.

## ARA1110 DUPLICATE_TYPE: Type name already registered
owner: ara-schema

A type name, type alias or document-kind name is already claimed. The conflicting registration is rejected; the existing definition remains registered.

Causes:
- Two project or built-in types use the same name or alias.
- A document kind is registered twice.

Fixes:
- Remove the duplicate registration or give the conflicting type or alias a unique name, then run `ara schema`.
- For runtime replacement, use `SchemaDb::remove` before adding the replacement definition; restart `ara serve` after project type-file changes.

## ARA1111 INVALID_PATH: Invalid field path
owner: ara-schema, ara-doc, ara-cmd

A field path is malformed, cannot be resolved against the schema or cannot be applied to the current value. The edit is rejected. `parent` and `order` cannot be changed through ordinary property writes.

Causes:
- A misspelled field, an invalid vector component or selection inside a scalar.
- Addressing a plain list by index instead of replacing it whole, or selecting a keyed item that does not exist.
- Using `property.set` for hierarchy fields or another field outside `name` and component data.

Fixes:
- Check `ara schema <Type>` and use a path such as `Transform.position.x` or a keyed collection's stable item ID.
- Replace a plain list as a whole with `property.set`; use `ara do entity.reparent` with `parent` and `index` for hierarchy changes.

## ARA1112 UPGRADE_FAILED: Type upgrade failed
owner: ara-schema

A component's upgrade hook returned an error while advancing stored data to the current version. Normalization stops for that component; the reader may retain partially transformed data, but the document has errors and must not be written back as a successful upgrade.

Causes:
- The stored value does not satisfy an upgrade hook's assumptions.
- A migration hook cannot convert the reported source version.

Fixes:
- Correct the source data or the type's upgrade hook using the version and reason in the message, then rerun `ara check <file>` from the original file.
- Run `ara fmt <file>` only after the upgrade succeeds without errors; do not merely raise the stored version number.

## ARA1113 NEWER_VERSION: Data version is newer
owner: ara-schema

The document schema version or stored component version is newer than this engine understands. An unsupported document kind version prevents loading; an unsupported component version leaves a parsed document with an error and no valid downgrade.

Causes:
- A newer engine or project schema wrote the file.
- The `$schema` URI or `versions` table was increased without a matching type definition.

Fixes:
- Open the file with the engine and project type definitions that support the recorded version, then run `ara check <file>`.
- If the version was edited accidentally, restore the original version and matching data together; do not lower a genuine newer version to bypass validation.

## ARA1114 INVALID_KEYED_ITEM: Invalid keyed collection item
owner: ara-schema

An item in a keyed array lacks a valid stable `id`, or repeats another item's ID. Validation fails because item paths and merges cannot identify it uniquely.

Causes:
- An item has no string `id`, a malformed ID or a duplicate ID within the collection.

Fixes:
- Give every item a distinct ID in the accepted form, such as `s_7c`, and preserve existing IDs when editing their values.
- Submit the corrected collection with `property.set`, or correct the file and run `ara check <file>`.

## ARA1115 UPGRADED: Type version upgraded
owner: ara-schema

A component was normalized from an older stored version to the current version. Available upgrade hooks ran successfully. This is informational; the transformed data is in memory and still needs writing to persist the upgrade.

Causes:
- The component's entry in `versions` is older than its current schema version; a missing entry is treated as version 1.

Fixes:
- Nothing to fix if no other errors remain; run `ara fmt <file>` to persist the upgraded values and current version table.

## ARA1201 UNKNOWN_SCHEMA: Unknown document schema
owner: ara-schema, ara-doc

The `$schema` value does not identify an understood document kind, or an operation uses an unregistered kind. The document or type definitions cannot be loaded through that schema; the affected edit is rejected.

Causes:
- A missing, non-string or malformed `$schema` URI.
- A kind that this engine has not registered, or the wrong schema for a `*.types.json` file.

Fixes:
- Use a supported URI from the diagnostic, such as `ara://schema/scene/1` for a scene, then run `ara check <file>`.
- For project type definitions, use `$schema: "ara://schema/types/1"`; register custom kinds before loading their documents.

## ARA1202 INVALID_ID: Invalid document or record ID
owner: ara-doc, ara-cmd

An ID is malformed or has a prefix inappropriate for its document or record kind. Invalid header IDs prevent document loading; invalid record IDs are reported and may be skipped. A command supplying an invalid ID is rejected.

Causes:
- A copied ID has uppercase letters, punctuation, an empty body or the wrong kind prefix.
- A command tries to create an entity with a non-entity ID.

Fixes:
- Let `entity.create` or `prefab.instantiate` mint an ID by omitting `id`, or supply a batch symbol such as `$crate`.
- For file repairs, use the prefix specified in the diagnostic and the grammar `<prefix>_<body>`: 1–3 lowercase prefix letters and 1–24 lowercase letters or digits in the body; update references consistently and run `ara check`.

## ARA1203 RECORD_EXISTS: Record already exists
owner: ara-doc, ara-cmd

A create operation targets an ID already present in the document. The existing record is preserved and the command batch is rejected.

Causes:
- Reusing a concrete record ID for a second entity.
- Repeating a creation request without an idempotency key.

Fixes:
- Omit `id` or use a fresh `$symbol` with `entity.create` so the engine allocates an unused ID.
- If the entity already exists, inspect it with `ara inspect` and change it through `property.set` instead of creating it again.

## ARA1204 RECORD_NOT_FOUND: Record not found
owner: ara-doc, ara-cmd, ara-host

The requested record, resolved prefab entity or name selector has no target. The read or edit fails; a failing command batch does not commit its other edits.

Causes:
- The entity was deleted, its name changed or the wrong document was selected.
- A nested prefab address no longer exists in the resolved instance.

Fixes:
- Find the current target with `ara query --doc <doc>` or `ara inspect --doc <doc>` and retry using its ID.
- Use a suggested selector when offered, and pass `--doc` explicitly if the same project has several scenes.

## ARA1205 MISSING_PARENT: Parent does not exist
owner: ara-doc

A record's parent is missing, malformed or addresses a child that does not exist in a resolved prefab. Document validation fails; a transaction introducing this error cannot commit.

Causes:
- A parent was deleted or its ID was mistyped.
- A slash-separated parent points inside a plain entity rather than a prefab instance.

Fixes:
- Use `ara do entity.reparent` to select an existing parent, or make the entity a root when appropriate.
- `ara check --fix <file>` can apply the offered removal of a missing `parent`; check that a prefab still has exactly one root afterwards.

## ARA1206 PARENT_CYCLE: Cyclic parent chain
owner: ara-doc, ara-cmd

Following parent links leads back to the same entity. The document hierarchy is invalid; reparenting under the entity itself or its descendants is rejected.

Causes:
- An entity was parented to itself or beneath one of its descendants.
- Independent edits formed a loop across several records.

Fixes:
- Use `ara do entity.reparent` to move one member of the cycle to a root or an unrelated parent.
- If repairing a file, break the cyclic `parent` link and run `ara check <file>` before formatting.

## ARA1207 INVALID_ORDER: Invalid sibling order
owner: ara-doc, ara-cmd

An `order` string is not a valid fractional index, or the command cannot generate a key between the chosen siblings. Validation or the placement command fails.

Causes:
- A hand-written order key has an invalid head, length, digit or trailing fractional zero.
- Sibling order keys do not leave a valid interval for the requested insertion.

Fixes:
- Use `ara do entity.reparent` with the desired parent and numeric `index` to generate order keys rather than setting `order` through `property.set`.
- Correct malformed file keys and reorder the affected siblings before retrying insertion; run `ara check <file>` to verify them.

## ARA1208 ROOT_COUNT: Invalid prefab root count
owner: ara-doc

A document kind requiring one root has zero or multiple root records. Prefab validation fails; expansion cannot produce an instance when the prefab has no root.

Causes:
- A prefab's root was deleted, or several entities were left without parents.
- Broken parent relationships leave no usable root.

Fixes:
- Choose one root and use `entity.reparent` to put the other top-level records beneath it, or restore the missing root.
- Run `ara check <prefab-file>` after repairing the hierarchy.

## ARA1209 UNRESOLVED_CONFLICT: Merge conflict still unresolved
owner: ara-doc

The document contains `$conflicts` entries from a semantic merge. Its records can be inspected, but it remains invalid and `ara fmt` will not silently erase those conflicts.

Causes:
- A previous `ara merge` left conflicting edits for a human or agent to choose between.

Fixes:
- Use `ara resolve <file> --pick <record>:<path>=ours` or the corresponding `theirs` or `base` choice; the diagnostic supplies concrete `ours` and `theirs` commands.
- To choose one side for all conflicts, use `ara resolve <file> --ours` or `--theirs`, then run `ara check <file>`.

## ARA1210 NOT_A_COMPONENT: Type is not a component
owner: ara-schema

A PascalCase record key names a registered type whose role is not a component. The record fails validation even though the type name itself exists.

Causes:
- A struct, enum or parameter type was placed directly on an entity as a component.

Fixes:
- Use `ara_schema` to list component types and attach a registered component with `component.add`.
- Put nested struct or enum data inside the component field that declares it, then run `ara check <file>`.

## ARA1211 INSTANCE_COMPONENTS: Inline component on prefab instance
owner: ara-doc

A prefab instance record carries a component directly beside its `prefab` reference. Instance data must express component changes through `overrides`; the document fails validation.

Causes:
- Component JSON was copied from a plain entity onto an instance record.

Fixes:
- Move the intended value into an override, using `ara do prefab.override.set` with the instance's `entity`, `path` and `value` when it is loaded.
- Remove the invalid inline component after preserving its intended values, then run `ara check <file>`.

## ARA1212 DANGLING_ENTITY_REF: Entity reference target missing
owner: ara-doc

A component's entity field names a record that is absent from the document. This is a warning: the document remains loadable, but the reference has no target. The check tests the leading record ID of a nested address.

Causes:
- The referenced entity was deleted or the reference contains a mistyped ID.

Fixes:
- Use `ara query` to find the intended entity and update the reference through `property.set`.
- If the reference is no longer needed, `ara check --fix <file>` applies the offered `null` value.

## ARA1213 MISSING_DOC_ID: Document asset ID missing
owner: ara-doc, ara-host

A document has no string asset `id`, so it cannot be loaded as an addressable document. An import sidecar without its asset ID cannot supply an index entry.

Causes:
- The top-level `id` was deleted, omitted or written as a non-string.
- An incomplete `.import.json` sidecar lacks its asset identity.

Fixes:
- Restore the original ID from the file's prior version so existing references continue to resolve.
- For a new document, assign an unused ID with the kind prefix shown in the diagnostic, then run `ara check`; preserve the ID on future edits.

## ARA1301 INVALID_REF: Malformed asset reference
owner: ara-schema, ara-doc

An asset reference does not follow the accepted syntax and cannot be resolved. The value fails validation; this is distinct from a well-formed reference to a missing asset.

Causes:
- A missing `@`, an empty reference or a malformed ID after `#`.
- An absolute path, backslashes, empty path segments or a `..` segment.

Fixes:
- Use a project-relative reference such as `@prefabs/crate.prefab#pf_91fd`, with `/` separators.
- Supply `@path` or `@#id` when only one half is known, then run `ara fmt <file>` to fill the other half from the asset index.

## ARA1302 UNRESOLVED_REF: Unresolved reference
owner: ara-doc

Neither the reference's path nor its ID is in the project's asset index. The original value is retained and an error is reported; the document cannot be safely formatted until it resolves.

Causes:
- The asset was deleted, moved without a usable ID, or never added to the project.
- Both halves are mistyped, or the target's document header or import sidecar is invalid and was not indexed.

Fixes:
- Restore the asset and its valid document header or import sidecar, then run `ara check`.
- Choose the intended reference from the diagnostic's suggestions; `ara check --fix <file>` applies it only when there is exactly one fix, otherwise use `property.set` or correct the file explicitly.

## ARA1303 REF_PATH_REPAIRED: Stale reference repaired
owner: ara-doc

A reference was repaired in memory using the half that still resolves. A known ID takes precedence over the supplied path; when the ID is unknown but the path exists, the path's current ID replaces it. This warning alone does not prevent loading.

Causes:
- An asset moved while retaining its ID.
- A valid path contains a stale or unknown asset ID.

Fixes:
- Confirm the repaired target is intended, then run `ara fmt <file>` to persist the full reference.
- `ara check --fix <file>` can also apply the supplied replacement value.

## ARA1304 REF_FILLED: Reference completed
owner: ara-doc

A path-only or ID-only asset reference resolved successfully and its missing half was filled in memory. This is informational; the resulting reference is canonical `@path#id`.

Causes:
- The input used `@path` or `@#id` as a shorthand reference.

Fixes:
- Nothing to fix in memory; run `ara fmt <file>` or `ara check --fix <file>` to write the completed reference permanently.

## ARA1305 REF_KIND_MISMATCH: Wrong asset kind
owner: ara-doc

The reference resolves, but the asset's indexed kind differs from the field's required kind. Resolution fails and the original field value is retained.

Causes:
- A prefab field points at a scene or source asset instead of a prefab.
- The ID resolves to a different kind than the path's appearance suggests; a known ID is authoritative.

Fixes:
- Inspect the required asset kind with `ara schema <Type>` and choose an asset of that kind.
- Set the correct full reference with `property.set`, or correct the file and run `ara check <file>`.

## ARA1401 PREFAB_NOT_FOUND: Prefab not loaded
owner: ara-doc, ara-cmd

The referenced prefab is not available in the document workspace. Instance expansion omits that instance's content, or `prefab.instantiate` rejects the command before creating a record.

Causes:
- The prefab file is missing or failed to load.
- Expansion received a reference without a resolvable loaded prefab ID.

Fixes:
- Restore the prefab and run `ara check <prefab-file>` to resolve its load errors.
- Use `ara fmt <scene-file>` to complete or repair its reference, then retry `prefab.instantiate` with a loaded prefab.

## ARA1402 PREFAB_CYCLE: Recursive prefab dependency
owner: ara-doc

Prefab expansion reached a prefab already on its expansion stack. The recursive branch is not expanded and validation reports an error instead of recursing indefinitely.

Causes:
- A prefab contains an instance of itself.
- Several prefabs instantiate each other in a loop.

Fixes:
- Break the dependency by removing or replacing one of the nested instances; use `entity.delete` for a loaded unwanted instance.
- Run `ara check` across the project to verify the entire prefab dependency chain.

## ARA1403 OVERRIDE_TARGET_MISSING: Override target missing
owner: ara-doc

An override targets an entity or value that is absent from the resolved prefab. That override cannot be applied; other instance content can still resolve, but the document has an error.

Causes:
- The source prefab removed or renamed the targeted entity or keyed item.
- A `removed` entry eliminated the entity before overrides were applied.
- The override's nested ID path is wrong.

Fixes:
- Remove the obsolete override with `ara check --fix <file>` using the supplied removal fix.
- If the change is still needed, inspect the current instance with `ara inspect` and apply `prefab.override.set` to the correct entity and path.

## ARA1404 INVALID_OVERRIDE_KEY: Invalid override key
owner: ara-doc

An override key has an invalid field-path syntax or tries to override a field other than `name` or component data. The override is skipped and the document has an error.

Causes:
- An override targets `parent`, `order` or another unsupported record field.
- The field-path portion after the final `/` is malformed.

Fixes:
- Use `ara check --fix <file>` to remove the invalid override, then recreate the intended change through `prefab.override.set`.
- Use `entity.reparent` on the instance for hierarchy changes; use `ara schema <Type>` for valid component field paths.

## ARA1405 REMOVED_TARGET_MISSING: Removed prefab target missing
owner: ara-doc

An instance's `removed` list names an entity or component that is not present when removals are applied. Nothing is removed for that entry, and validation reports an error.

Causes:
- The source prefab already deleted the target.
- A duplicate removal, or removal of an ancestor first, made the later target absent.
- The removal entry contains the wrong entity address or component name.

Fixes:
- Run `ara check --fix <file>` to apply the offered replacement `removed` list without the obsolete entry.
- Inspect the resolved instance with `ara inspect` before issuing a new `entity.delete` or `component.remove` command against it.

## ARA1406 NOT_AN_INSTANCE: Entity is not a prefab instance
owner: ara-doc, ara-cmd

A prefab-only operation or nested entity address was used on a plain record. The command is rejected. A plain record carrying `overrides` or `removed` also fails document validation.

Causes:
- `prefab.override.set` or `prefab.override.revert` targeted an ordinary entity.
- A slash-separated address starts with an entity that has no prefab reference.
- Prefab-only fields were left on a plain record.

Fixes:
- Use `ara do property.set` for an ordinary entity, or target an actual instance created with `prefab.instantiate`.
- Run `ara check --fix <file>` to remove `overrides` or `removed` when the diagnostic offers that repair for a plain record.

## ARA1501 STALE_BASE: Transaction revision conflict
owner: ara-ops

The transaction cannot commit against its stated revision. A later transaction touched an overlapping path, or the supplied base revision is ahead of the store. Journal restoration also rejects a transaction whose result revision is not newer than the store. A rejected transaction leaves no committed changes.

Causes:
- A client edited a field using a revision from before another edit to that field or record.
- A revision from another store was reused, or journal transactions arrived out of order.

Fixes:
- Read `session.status`, inspect the current entity with `ara_inspect` and reconcile the conflicting value before retrying `ara_apply` against the current revision.
- Use `ara history` to identify the transaction named in the message; for journal replay failures, investigate the accompanying `ARA1905` instead of resubmitting out-of-order transactions.

## ARA1502 UNDO_CONFLICT: Undo would overwrite a later edit
owner: ara-ops

Undo overlaps a path changed after the target transaction. The inverse transaction is rejected rather than overwriting the later edit; the current document remains unchanged by this undo.

Causes:
- Another transaction modified the same field, component or record after the edit being undone.

Fixes:
- Inspect `ara history` and the current value with `ara inspect`, then make the intended correction explicitly through `property.set`.
- If the later edits should also be undone, use `ara undo <txn>` on those transactions in reverse order before retrying the original undo.

## ARA1503 ALREADY_REVERTED: Transaction already reverted
owner: ara-ops

The requested transaction already has a recorded inverse. A second revert is rejected and makes no further changes; the message identifies the reverting transaction.

Causes:
- An undo or revert request was repeated after it succeeded.

Fixes:
- Check `ara history`; if the change is already undone, no further action is needed.
- Use `ara redo` when the intention is to restore an eligible undone change for the current author.

## ARA1504 TXN_NOT_FOUND: Transaction not found
owner: ara-ops

The store has no transaction with the supplied ID. Undo cannot construct an inverse and makes no changes.

Causes:
- A mistyped transaction ID or one from another project.
- The transaction is no longer in the retained journal history.

Fixes:
- Run `ara history` and retry `ara undo <txn>` with an ID from this project's retained history.
- If the old transaction is unavailable, inspect the current document and apply the desired correction with `ara_apply`.

## ARA1505 NOTHING_TO_UNDO: No undo or redo available
owner: ara-ops

The selected author has no eligible transaction on the requested undo or redo stack. No transaction is created and documents are unchanged.

Causes:
- That author has made no undoable edit, or all eligible edits have already been undone.
- Redo was requested without an eligible preceding undo.

Fixes:
- Check `ara history --by <author>` and the CLI's `--author` or `ARA_AUTHOR` setting.
- If there is no eligible change, nothing needs fixing; use `ara undo <txn>` only when a specific retained transaction is the intended target.

## ARA1506 DOC_NOT_FOUND: Document not loaded
owner: ara-ops, ara-cmd, ara-host

The workspace cannot find the requested document by ID or path, or `doc.sync` has no file-backed document to synchronize. The request fails without committing its edits.

Causes:
- A mistyped document selector, wrong project or deleted document.
- The file was not loaded because of validation errors or because it lies outside the project scan.

Fixes:
- Use the `documents` returned by `session.begin` and pass the exact path or ID to `ara_apply`, `ara_inspect` or CLI `--doc`.
- Run `ara check <file>` to fix load errors and ensure the file is inside the intended project; use `ara --root <project-dir>` when necessary.

## ARA1507 INVALID_AUTHOR: Invalid author identity
owner: ara-ops

The author string cannot be parsed. Session creation or the operation requiring that identity is rejected before it can attribute changes.

Causes:
- A value other than `human` or `agent:<name>`.
- An empty agent name or characters outside ASCII letters, digits, `-`, `_` and `.`.

Fixes:
- Set the correct identity in `session.begin`, CLI `--author` or `ARA_AUTHOR`, for example `agent:codex` for that agent.
- Keep agent actions attributed to the agent; use `human` only for a human caller.

## ARA1601 UNKNOWN_COMMAND: Unknown command or job
owner: ara-cmd, ara-host, ara-script

The registry or loaded C# command catalog has no entry with the requested name. The batch or job request is rejected; the missing command is not executed.

Causes:
- A typo in the command or job name.
- The command's script module or providing feature is not loaded.

Fixes:
- Use `ara_schema` with `action: "list"` and `kind: "command"` or `kind: "job"` to find available names.
- Correct the name suggested by the diagnostic, or load the defining module before retrying `ara_apply` or `ara_jobs`.

## ARA1602 INVALID_PARAMS: Invalid operation parameters
owner: ara-cmd, ara-host, ara-mcp, ara, ara-script, ara-editor, ara-net

Parameters cannot be decoded or violate an operation's rules. The request is rejected rather than applied with guessed values. This code also reports invalid command examples, module metadata and some service configuration failures; the message identifies the affected input.

Causes:
- Missing or incompatible command arguments, malformed CLI `--args`, or an unsupported MCP `action`.
- Invalid batch `as` usage, a review request with no target, or reparenting an entity inside a prefab instead of its instance.
- A command implementation's example, output or script descriptor cannot be serialized or decoded.

Fixes:
- Inspect the method or command with `ara_schema` or MCP `tools/list`, then supply its documented parameter object and action.
- For shell quoting problems, use `ara do <command> --args @file.json` or `-p key=value`; for batches, pass `{commands: [{cmd, args}], why, doc}`.
- If the error names implementation metadata rather than caller input, correct the registered command example, descriptor or result type before retrying.

## ARA1603 MISSING_SCOPE: Required capability missing
owner: ara-cmd, ara-host

The session lacks the scope required by a method or command. The request is rejected before that operation runs. `daemon.shutdown` additionally requires a human session.

Causes:
- The scope was not requested or was refused by the approval policy.
- An agent session attempted to stop the daemon.

Fixes:
- Inspect `session.status`, then request the needed scope through `session.begin` under the project's approval policy and use the newly granted session handle.
- If policy denies the scope, have an authorized operator approve or perform the operation; a human should use their own session for `daemon.shutdown`.

## ARA1604 UNRESOLVED_SYMBOL: Undefined batch symbol
owner: ara-cmd

An entity parameter uses a `$symbol` that no earlier command in this batch has bound. The batch is rejected atomically.

Causes:
- A reference appears before its creating command.
- A symbol is misspelled or was defined in a different batch.

Fixes:
- Put `entity.create` or `prefab.instantiate` first with `id: "$crate"` or `as: "crate"`, then reference `$crate` in later commands of the same `ara_apply` batch.
- Across batches, use the concrete ID returned by the successful creation rather than its temporary symbol.

## ARA1605 SYMBOL_REDEFINED: Batch symbol defined twice
owner: ara-cmd

Two creations in one batch try to bind the same symbolic ID. The batch is rejected, including its earlier staged edits.

Causes:
- Copying a create command without changing its `id` or `as` symbol.

Fixes:
- Give each creation a distinct symbol and update subsequent references before retrying `ara_apply`.
- To edit the entity already bound to that symbol, issue `property.set` instead of another creation.

## ARA1606 COMPONENT_EXISTS: Component already present
owner: ara-cmd

`component.add` found that the target already has the component, including components inherited from a prefab. The batch is rejected and the existing component is preserved.

Causes:
- A repeated add request or an inherited component mistaken for a missing one.

Fixes:
- Use `ara do property.set` to change the existing component's fields.
- Inspect the entity with `ara inspect` before deciding whether a new component is needed.

## ARA1607 COMPONENT_MISSING: Component not present
owner: ara-doc, ara-cmd

The command tries to remove or edit a component the target does not have. The operation is rejected instead of implicitly adding a component.

Causes:
- `property.set` targets a component that was never added or was removed.
- A repeated `component.remove` request.

Fixes:
- Add the component with `component.add` before setting its fields; both commands can be ordered in one `ara_apply` batch.
- For a removal, inspect the entity with `ara inspect`; omit the removal if the component is already absent.

## ARA1608 WRONG_KIND: Wrong registry entry kind
owner: ara-host

`jobs.start` found the named registry entry, but it is not a job. No job is started.

Causes:
- A command or query name was passed as the `job` parameter.

Fixes:
- Use `ara_apply` / `cmd.apply` for a command, as directed by the diagnostic.
- List actual jobs with `ara_schema` using `action: "list"` and `kind: "job"`, then pass one of those names to `ara_jobs` with `action: "start"`.

## ARA1609 AMBIGUOUS_ENTITY: Entity selector is ambiguous
owner: ara-cmd, ara-host

A name selector matches multiple resolved entities in the selected document. The engine refuses to guess: inspection or the edit batch fails and lists candidate targets through fixes.

Causes:
- Several entities share a display name.
- A short name path does not distinguish repeated prefab instances or hierarchy branches.

Fixes:
- Choose a concrete entity ID from `ara query` or the diagnostic's suggested replacements.
- Use a longer selector such as `name:Floor/Crate`, and pass `--doc` when the project contains several documents.

## ARA1610 MISSING_WHY: Agent change needs a reason
owner: ara-host

An agent submitted `cmd.apply` without a nonempty `why`. The host dry-runs the batch to report other problems, then rejects it without committing changes, so the future reviewer is not left with an unexplained edit.

Causes:
- The request omitted `why` or supplied only whitespace.
- The CLI detected an agent identity from its environment, but the command omitted `--why`.

Fixes:
- Retry `ara do <command> --why "<reason for the change>"`, or add a meaningful sentence in `ara_apply`'s `why` field.
- Fix any additional diagnostics returned with the dry run before resubmitting the batch.

## ARA1701 MERGE_CONFLICT: Semantic merge conflict
owner: ara-merge

A semantic three-way merge could not reconcile competing values, a deletion against an edit, or a newly invalid combined structure. The merge result retains `$conflicts` entries and usable records for resolution; `ara merge` writes that result but reports failure while conflicts remain.

Causes:
- Both sides changed the same field differently.
- One side deleted a record or keyed item that the other changed.
- Individually valid changes combine into an invalid hierarchy or document structure.

Fixes:
- Use `ara resolve <file> --pick <record>:<path>=ours`, `theirs` or `base` for the reported conflict.
- Run `ara check <file>` after resolution, then `ara fmt <file>` to canonicalize and repair references.

## ARA1702 MERGE_ID_MISMATCH: Merge document identity mismatch
owner: ara-merge

The `ours` and `theirs` inputs have different asset IDs or document kinds. The merge is rejected without producing a merged document; unrelated assets must not be combined as revisions of one document.

Causes:
- Inputs from different scene or prefab files were supplied to `ara merge`.
- One side changed the document's asset ID or kind.

Fixes:
- Supply `ara merge <base> <ours> <theirs>` with revisions of the same asset.
- Restore an accidentally changed identity from the correct prior version; keep intentionally distinct assets as separate documents.

## ARA1801 INVALID_REQUEST: Invalid protocol request
owner: ara-proto, ara-host, ara-rpc

A message does not satisfy JSON-RPC framing or the host's object-parameter requirement, so the request is rejected. This code also represents a refused non-local browser origin, or wraps an external RPC error that supplied no structured Ara diagnostics; read its message for that underlying failure.

Causes:
- A missing `jsonrpc: "2.0"`, invalid message shape or non-object method parameters.
- An HTTP request has a browser origin outside the allowed loopback hosts.
- A transport or foreign server returned an error without `data.diagnostics`.

Fixes:
- Send a JSON-RPC 2.0 object with a method name and object `params`, using `rpc.discover` for the method's schema.
- Use `ara mcp` or the supported authenticated local endpoint for local clients; diagnose the underlying transport message when this code wraps another error.

## ARA1802 UNKNOWN_METHOD: Unknown protocol method
owner: ara-host

No registered Engine Protocol route handles the requested method, or a service cannot handle a method routed to it. The call fails without executing that method.

Causes:
- A typo or a method name from a different engine version.
- A service's method catalog and dispatcher disagree.

Fixes:
- Call `rpc.discover` and use a method listed by the running host; apply the spelling suggestion when appropriate.
- Use the supported MCP tools such as `ara_apply` and `ara_inspect`, or correct a service's registration and dispatcher if they disagree.

## ARA1803 SESSION_NOT_FOUND: Live session not found
owner: ara-host

A session-required operation received no live session handle. The handle may be absent, unknown or ended; the requested operation is rejected.

Causes:
- The caller omitted `session` or reused a handle after `session.end`.
- A handle belongs to another project or is no longer present in this host's session state.

Fixes:
- Call `session.begin` with your client name, or `ara_session` with `action: "begin"`, and pass its returned `session` on subsequent calls.
- For a per-connection stdio MCP client, let the adapter reuse its current session or begin a new one after ending it.

## ARA1804 UNKNOWN_SCOPE: Unknown capability scope
owner: ara-proto

A requested scope is not in the protocol's scope vocabulary. Session creation fails before any approval decision or new session is recorded.

Causes:
- A typo such as `edit:scen`, or a scope name not supported by this engine.

Fixes:
- Read `rpc.discover`'s `scopes` list and retry `session.begin` with supported names such as `read` and `edit:scene`.
- Use the spelling suggestion in the diagnostic; omitting `scopes` requests the approval policy's defaults.

## ARA1805 SCOPE_DENIED: Requested scope denied
owner: ara-host

The approval policy withheld some requested scopes. This is a warning, not a failed session start: the host created the session with its granted subset and returned both `granted` and `denied`.

Causes:
- The requested capabilities exceed the policy for this caller; the default agent policy excludes `generate:$`, `fs:outside-project` and `net`.

Fixes:
- Use `session.status` or the `session.begin` result to check which operations the session can perform.
- If a denied capability is necessary, have the project operator adjust approval policy and begin a new session; otherwise continue within the granted scopes.

## ARA1806 SERVICE_UNAVAILABLE: Protocol service unavailable
owner: ara-host

The method belongs to a reserved protocol family whose providing service is not registered in this engine build. The request fails; an advertised placeholder MCP tool does not imply an implementation is present.

Causes:
- A client called a reserved family such as `run`, `capture`, `play` or `test` before its service was registered.

Fixes:
- Check `rpc.discover` for the methods actually registered by the current host.
- Use a host build that registers the provider named in the message, then refresh MCP `tools/list` before retrying.

## ARA1807 UNAUTHORIZED: Project token rejected
owner: ara-rpc

The HTTP or WebSocket connection did not present the current project token. The server refuses it with HTTP 401 before dispatching the engine request.

Causes:
- A missing or incorrect bearer token.
- A client reused a token from another project or an earlier token file.

Fixes:
- Read the local project's `.ara/token` and authenticate with `Authorization: Bearer <token>` on the connection; keep the token out of logs and source control.
- Use `ara mcp` or the Ara CLI to connect through the project's daemon discovery instead of constructing authentication manually.

## ARA1808 INVALID_QUERY: Invalid entity query
owner: ara-host

The compact query or structured filter contains an unsupported component, field path or condition. Query validation fails before returning matching entities.

Causes:
- An unknown component in `has` or `lacks`, or an invalid comparison field path.
- A `depth` condition other than `depth<N` or `depth<=N` with an unsigned integer.
- Mixing bare entity-name words with an explicit `name:` condition.

Fixes:
- Check component and field names with `ara_schema`, then retry `ara query` or `ara_query`; the diagnostic may supply a corrected `q` value.
- Use one name expression and supported conditions, for example `ara query "Light name:Crate* Light.intensity>500"`.

## ARA1809 INVALID_CURSOR: Invalid pagination cursor
owner: ara-host

The pagination cursor cannot be decoded as the host's `c`-prefixed numeric offset. The listing request fails. This check detects malformed cursors, but does not detect a valid cursor reused with another query.

Causes:
- A cursor was truncated, edited or copied from another API's format.

Fixes:
- Restart the listing without `cursor`, then pass back the exact cursor returned by that method.
- Keep the same query and filters while paging so the offset continues to refer to the intended result set.

## ARA1810 CHECKPOINT_NOT_FOUND: Session checkpoint not found
owner: ara-host

The current session has no checkpoint with the supplied ID. A checkpoint-based diff or rollback is rejected; rollback makes no changes.

Causes:
- The checkpoint belongs to another session or its ID is mistyped.
- The caller used a transaction ID where a checkpoint ID was required.

Fixes:
- Inspect the session's checkpoints with `ara_session` using `action: "status"` and select one belonging to that session.
- Create a new boundary with `session.checkpoint` for future work, or use the automatic checkpoint returned by a successful agent `ara_apply` call.

## ARA1811 JOB_NOT_FOUND: Job handle not found
owner: ara-host

The job table has no execution with the supplied handle. Status lookup or cancellation fails; no other job is affected.

Causes:
- A mistyped job handle or a job name used instead of the returned execution ID.
- The host restarted and the old in-memory job handle is no longer available.

Fixes:
- Call `ara_jobs` with `action: "list"` and use a returned job ID for `status` or `cancel`.
- When starting new work with `ara_jobs` and `action: "start"`, retain the returned handle for subsequent requests.

## ARA1812 SESSION_APPROVED: Session approved
owner: ara-host

A session was created and journaled. This informational log records its client, author, granted scopes and approval authority; separately denied scopes may still be reported by `ARA1805`.

Causes:
- A successful `session.begin`, including one created automatically by a CLI or per-connection MCP client.

Fixes:
- Nothing to fix; retain the returned session handle and inspect `session.status` when you need its granted scopes or checkpoints.
- End the session with `session.end` when the work is finished; its approval is already recorded in the journal.

## ARA1813 NOT_REVIEWABLE: Change cannot be reviewed
owner: ara-host

No eligible review target matched, or an agent tried to accept changes. The accept or revert request makes no changes to review state or documents.

Causes:
- The transaction is not a tracked agent transaction, or its matching entities have no eligible review state left.
- The requested entity does not match a reviewable change.
- `review.accept` was called from a non-human session.

Fixes:
- Use `ara review` or `review.list` to select a currently reviewable transaction or entity.
- Have a human review and accept with their own session; agents can use `review.revert` or undo their changes instead of accepting them.

## ARA1814 DOC_REQUIRED: Document must be specified
owner: ara-host

The call omitted `doc` and the host could not infer one unambiguously from its entity references, a single scene or the project's main scene. The inspect or apply request is rejected before operating on a document.

Causes:
- The addressed entities occur in multiple documents.
- There is no usable main scene and the workspace does not have exactly one scene.

Fixes:
- Choose one of the diagnostic's document suggestions and pass `--doc <path>` to `ara do` or `ara inspect`, or `doc` to `ara_apply` or `ara_inspect`.
- Use the `documents` and `mainScene` returned by `session.begin` to select the intended document explicitly.

## ARA1901 NO_PROJECT: Ara project not found
owner: ara-host

Project discovery found no `ara.project.json` in the selected directory or its ancestors. The project-dependent command cannot run and no project is implicitly created.

Causes:
- The command was launched outside a project or with the wrong `--root`.
- The project marker is missing.

Fixes:
- Run the command inside the intended project, or pass `ara --root <project-dir>`.
- For a new project, run `ara init <dir>` using the diagnostic's suggested command; restore an accidentally deleted marker for an existing project.

## ARA1902 PROJECT_EXISTS: Project already exists
owner: ara

`ara init` found an existing `ara.project.json` in the target directory and stopped before writing its project template. The existing project is preserved.

Causes:
- Initialization was repeated in an existing project directory.

Fixes:
- Use the existing project with `ara --root <dir>` and run `ara check` to inspect it.
- To create a separate project, choose a different directory with `ara init <new-dir>`.

## ARA1903 DAEMON_RUNNING: Project lock already held
owner: ara-host

Another process holds the project's `.ara/lock`. A second host cannot open the project; the current owner may be a daemon or a short-lived CLI command.

Causes:
- Another `ara serve`, `ara mcp` or offline command is already serving or editing the project.

Fixes:
- Use normal CLI or MCP commands to connect to the existing daemon, or wait for the other offline command to finish.
- To replace the daemon, have a human session call `daemon.shutdown` and wait for it to release the lock before starting `ara serve`; deleting the lock file is not a substitute for stopping its owner.

## ARA1904 DAEMON_UNREACHABLE: Daemon connection failed
owner: ara-rpc

The daemon connection was lost or the client did not receive the requested response before its timeout. The client cannot confirm the call's result; a request sent before disconnection may already have committed.

Causes:
- The daemon exited, restarted or closed the connection.
- The daemon did not answer within the client's 120-second call timeout.

Fixes:
- Reconnect through the project's CLI or `ara mcp`; start `ara serve` if no daemon is running.
- Before repeating an edit, check `ara history` and `ara inspect` for its result; reuse the original `cmd.apply` idempotency key when retrying a keyed request.

## ARA1905 JOURNAL_CORRUPT: Journal recovery problem
owner: ara-host

A malformed `.ara/journal` line is skipped with a warning. If a decoded transaction cannot be restored, recovery logs an error and stops restoring later transactions; earlier recovered state is retained. The message distinguishes these cases.

Causes:
- A truncated or malformed journal line.
- A transaction's revision or operations no longer fit the restored store or available documents.

Fixes:
- Preserve a copy of `.ara/journal` before investigating the reported line or transaction; compare `ara history`, `ara logs` and current documents to identify what was recovered.
- Restore missing documents or recover the intended changes from a valid backup, then verify them with `ara check`; do not assume skipped or later transactions were applied.

## ARA1906 JOURNAL_REPLAYED: Unsaved transactions recovered
owner: ara-host

The host replayed transactions that were journaled but had not reached their document files. They are now in memory and marked for flushing; the message reports the number recovered.

Causes:
- The previous host stopped before its debounced document writes completed.

Fixes:
- Nothing to fix; let the host finish saving and inspect `session.status` for file state.
- Use `ara history` or `ara inspect` to confirm the recovered changes, and stop the host cleanly so its final flush can complete.

## ARA1907 EXTERNAL_EDIT: External document change observed
owner: ara-host

A new document was loaded, or an external edit was merged into the loaded document as a human transaction. At startup this code also reports a file changed while no host was running: disk is kept as authoritative, and a warning explicitly says when newer journaled edits were not replayed onto it.

Causes:
- Another editor saved a valid document or created a new one.
- A file changed while the host was stopped and no longer matches its journaled flush hash.

Fixes:
- Nothing to fix for an expected clean change; inspect it with `ara inspect` and `ara history`.
- If the message says journaled edits were not replayed, compare the disk document and retained history, then reapply only the missing intended changes with `ara_apply`.

## ARA1908 EXTERNAL_CONFLICT: Disk edit conflicts with memory
owner: ara-host

A disk change conflicts with unsaved in-memory edits during three-way reconciliation. Neither version is overwritten. The host marks the document diverged and suspends automatic flushes for it until the conflict is resolved.

Causes:
- The host and an external editor changed the same field differently, or made incompatible structural changes since the last synchronized version.

Fixes:
- Reconcile the external file so it can merge cleanly, then let the watcher process the corrected edit.
- To deliberately choose a whole version, call `doc.sync` with `doc` and `keep: "memory"` to overwrite disk, or `keep: "disk"` to replace loaded values from disk; inspect both versions before choosing.

## ARA1909 EXTERNAL_INVALID: External file is invalid
owner: ara-host

A new or changed file cannot be accepted as a valid document. A new document is not loaded; for an already loaded document, the host keeps its previous in-memory version and suspends writes to the diverged file.

Causes:
- The external edit contains JSON, schema, reference or structural errors.
- A new file reuses the asset ID of an already loaded document.

Fixes:
- Run `ara check <file>`, fix the reported errors or duplicated asset identity, and save again so the watcher can load it.
- To discard an invalid disk edit deliberately, call `doc.sync` with the loaded `doc` and `keep: "memory"`; this overwrites the external file with the retained document.

## ARA1910 DOC_DELETED: Document deleted on disk
owner: ara-host

A loaded file disappeared. If memory matched the last synchronized version, the host unloads the document and logs an informational message. If unsaved changes exist, it keeps them in memory, marks the document diverged and warns instead of recreating the file automatically.

Causes:
- An external editor, rename or filesystem operation removed the document path.

Fixes:
- For retained unsaved changes, call `doc.sync` with `keep: "memory"` to recreate the file, or `keep: "disk"` to accept the deletion and unload it.
- If the already-unloaded deletion was accidental, restore the file at its project path and let the host scan load it again; otherwise nothing needs fixing.

## ARA1911 FLUSH_FAILED: Document save failed
owner: ara-host

The host could not atomically write a changed document to disk. Its in-memory changes remain, and the document is put back on the pending-write list for a later flush. The file has not been confirmed saved at the current revision.

Causes:
- Write permissions, a locked file, missing access or insufficient disk space blocked the temporary-file write or rename.

Fixes:
- Restore write access and disk space for the named path, then allow the running host to retry its pending flush.
- Check `session.status` and `ara logs` to confirm the file is saved before closing the host.

## ARA1912 TYPES_CHANGED: Project type definitions changed
owner: ara-host

A watched `*.types.json` file changed on disk. The running daemon keeps its existing Schema DB; it reports this warning rather than hot-reloading project types.

Causes:
- A project's type-definition file was modified or removed after the host started.

Fixes:
- Run `ara schema` to validate the definitions on disk, then restart the daemon with `ara serve` to load them.
- Refresh `ara_schema` or MCP `tools/list` after reconnecting so subsequent edits use the new schema.
