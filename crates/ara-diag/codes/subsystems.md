# Subsystems: graphs, materials, terrain, VFX, 2D, physics, navigation, localization, saves (ARA3100–ARA3699, ARA7000–ARA7599)

## ARA3100 graph::STRUCTURE: Invalid graph structure
owner: ara-graph

Graph storage, identity or registry metadata is invalid. Decoding or the requested command fails; validation alone reports problems without rewriting the graph.

Causes:
- The document is not a graph, lacks `g_meta`, or contains unexpected records or malformed field data.
- A node has an invalid `n_` ID or zero version, a subgraph reference lacks its path or ID, or the header names a different domain adapter.
- A registry entry repeats a kind/version or lacks types or documentation; a command supplies the wrong ID prefix, an empty parameter key or a nonfinite position.

Fixes:
- Correct the reported record or command argument: use `n_` node IDs, `w_` edge IDs, positive versions and full `@path#id` subgraph references.
- Load the matching graph and domain adapter; apply snapshots only to the graph with the same asset ID.
- Give each registered kind/version unique, documented metadata and use finite coordinates with `graph.node.move`.

## ARA3101 graph::UNKNOWN_NODE: Unavailable graph node
owner: ara-graph

The node's exact kind/version is unavailable, so domain validation or material projection cannot interpret it. The graph retains the node's original payload for inspection and repair.

Causes:
- The plugin supplying the node kind or its saved version is absent from `NodeRegistry`.
- A material graph contains a node kind or custom function that its material adapter does not recognize.

Fixes:
- Register or install the implementation for the exact kind/version named in the message.
- Replace the unsupported node with a supported one using `graph.node.add`, reconnect its consumers, then remove the old node with `graph.node.remove`.

## ARA3102 graph::PORT: Invalid graph connection
owner: ara-graph

A node socket or edge violates the graph's wiring rules. Validation leaves the graph available for repair; a rejected `graph.connect` call does not add the invalid edge.

Causes:
- An edge refers to a missing socket, connects the wrong directions or joins incompatible types.
- An input already has a connection, or a required input has neither a connection nor a parameter value.
- Socket IDs, types or documentation are empty, or fixed sockets differ from the registered node version.

Fixes:
- Use `graph.disconnect` on the offending edge, then `graph.connect` with existing, compatible output and input sockets.
- Connect the reported required input or supply its supported parameter value with `graph.param.set`.
- Restore the node's sockets to its registered version, including stable port keys, types and documentation.

## ARA3103 graph::CYCLE: Cyclic graph dependency
owner: ara-graph

The graph cannot be topologically ordered, or its domain forbids feedback. The authored graph is retained, but compilation requiring an acyclic graph cannot proceed. Generic graph commands permit feedback; material expressions must form a directed acyclic graph.

Causes:
- An expression directly or indirectly consumes its own output.
- A feedback edge was authored in a generic graph and then checked by a domain that forbids cycles.

Fixes:
- Remove an edge in the dependency loop with `graph.disconnect`, then revalidate the domain graph.
- For materials, edit the expression or use `material.connect` to point the argument at an independent source.

## ARA3200 material::SYNTAX: Material syntax error
owner: ara-material

Material text or its graph representation cannot be parsed or normalized correctly. A parsing failure rejects the text edit; structural validation errors prevent a checked material from being generated.

Causes:
- The text has an unexpected token, an unterminated string or an invalid declaration.
- Binding names repeat or use reserved identifiers, material identity is not `gr_`, or node IDs overlap or misuse reserved `n_output`.
- A graph projection uses edge IDs that were not derived from destination sockets.

Fixes:
- Correct the text at the reported line and column, then submit it with `material.text.apply`.
- Use unique, nonreserved binding names and retain the graph's existing asset and node identities.
- Rewire material graphs with `material.connect` so destination-derived edge identities remain valid.

## ARA3201 material::TYPE: Material type mismatch
owner: ara-material

A literal, parameter, call or output does not match its declared type. Authoring commands can retain some invalid drafts for correction, but shader generation requires a successful type check.

Causes:
- A value has the wrong lane count, is nonfinite or cannot fit in `f32`.
- Call arguments do not match the operation signature, or the declared result type differs from the inferred type.
- A domain output has the wrong type, or a numeric value is used where a boolean or symbolic texture is required.

Fixes:
- Apply the attached `material.node.type`, `material.parameter.reset` or `material.output.default` command when its proposed correction matches your intent.
- Correct argument types and use explicit color conversion such as `linear(encodedColor)` where required.
- Use the attached `material.node.literal` fix to replace an invalid expression with a typed zero, or correct the literal in `material.text.apply`.

## ARA3202 material::REFERENCE: Missing material reference
owner: ara-material

A material expression, output, function or command refers to an unavailable binding or operation. The affected material cannot pass checking or projection; a command targeting a missing node or field is rejected.

Causes:
- A call argument or output points at a deleted or absent binding.
- An operation or custom function name is unknown.
- A custom function lacks a unique nonreserved name, valid input names, documentation or a nonempty body.

Fixes:
- Restore the missing declaration or reconnect the consumer with `material.connect` to an existing node and port.
- Correct the operation name or add the custom function definition in `material.text.apply`.
- Give custom functions unique names, distinct identifier inputs, documentation and a Slang body; retry commands against current node IDs.

## ARA3203 material::DOMAIN: Invalid material domain usage
owner: ara-material

The material uses an operation or output unavailable in its domain, or a projection received a non-material graph. Domain checking fails and no checked shader can be generated; a parseable authoring draft may remain available for repair.

Causes:
- An operation is restricted to another domain, such as a UI-only operation used in a surface material.
- The material has no outputs or names an output that its domain does not expose.
- A graph supplied to the material adapter belongs to another domain.

Fixes:
- Edit the material text to use operations and output fields supported by its declared domain, then apply it with `material.text.apply`.
- Add at least one valid domain output, or select the intended material domain and update its outputs accordingly.
- Send the material adapter a material graph rather than another graph document.

## ARA3204 material::VARIANT_BUDGET: Material variant budget exceeded
owner: ara-material

The Cartesian product of static boolean switches exceeds `variant_budget`. Each static switch doubles the declared combinations, including switches not currently selected by an instance. Checking blocks shader generation; the authoring draft may remain editable.

Causes:
- Too many parameters are declared `static` for the configured budget.
- The budget was reduced below the material's declared combination count.

Fixes:
- Make parameters ordinary runtime parameters where compile-time specialization is unnecessary.
- After reviewing compile cost, apply the attached `material.budget.set` command to raise the budget to the reported combination count.

## ARA3205 material::OVERRIDE: Invalid material override
owner: ara-material

An instance override, static switch or instance document violates the material contract. Validation, variant selection or parameter packing fails; invalid overrides are not silently converted to valid values.

Causes:
- An override targets an unknown parameter or uses the wrong type; textures contain numeric heap slots instead of symbolic assets or defaults.
- A static switch is not boolean, or an instance document has malformed data or an asset ID without the `mi_` prefix.
- The renderer resolves a texture to an index outside the supported 16-entry table.

Fixes:
- Apply the attached `material.instance.clear` command for the reported node to restore the definition default.
- Supply a correctly typed value, use symbolic texture assets or defaults, and declare static switches as `bool`.
- For allocation errors, correct the renderer's texture allocation to indices `0..15`; do not put heap indices into authored overrides.
- Repair malformed instance data and use a valid `mi_` asset ID.

## ARA3206 material::COMPILER: Material shader compilation failed
owner: ara-material

The external Slang compiler could not produce a valid target artifact. No replacement shader is returned; the host is responsible for retaining its last good shader and publishing successful replacements at an allowed frame boundary.

Causes:
- `slangc` cannot be launched, or its `-version` probe fails.
- Generated or custom Slang fails compilation for SPIR-V or DXIL.
- Temporary source or output files cannot be written or read, or the output has an invalid target header or length.

Fixes:
- Set `SLANGC` to the compiler executable, or set `VULKAN_SDK` so its `Bin` directory supplies `slangc.exe` on Windows; verify `slangc -version` using that executable.
- Correct the Slang error reported in the compiler output, using generated source mappings to locate the material node or custom function.
- Restore access to the temporary directory and retry the offline cook with a working compiler for the requested target.

## ARA3207 material::LEGACY_SYNTAX: Legacy material syntax
owner: ara-material

The material uses rejected draft syntax. Parsing returns diagnostics rather than accepting the draft, and a recoverable draft receives a `material.text.apply` source replacement. Texture-slot migration chooses a semantic fallback because it cannot infer the original asset.

Causes:
- Material headers or declarations embed draft asset/node IDs, or values use array literals or numeric texture slots.
- Static parameters use explicit `variants` arrays, custom calls use `custom.` prefixes, or cost declarations use positional arrays.

Fixes:
- Review and apply the attached `material.text.apply` replacement; replace any suggested texture fallback with the intended symbolic asset.
- Use name-based declarations, scalar or `vec`/`mat`/`linear`/`srgb` constructors, and `static param name : bool = true;` for switches.
- Call custom functions by name and write costs as `cost(alu: N, samples: N, branches: N)`.

## ARA3208 material::PLAIN_RENAME: Possible material text rename
owner: ara-material

Text reconciliation removed an old name while introducing new names of the same role. This is a warning: the reconciled material is returned, but a plain text rename creates new node identity and may lose associated layout or override identity. The diagnostic offers possible identity-preserving recoveries.

Causes:
- A parameter or expression was renamed directly in text instead of with `material.rename`.
- An intentional deletion and addition resembles a rename.

Fixes:
- If this was a rename, apply the matching attached `material.rename` recovery, retaining its diagnostic-provided `restoreId`.
- For future renames, use `material.rename` with `from` and `to` so identity, connections, layout and overrides survive.
- If deletion and creation were intentional, keep the new identity and disregard the rename suggestion.

## ARA3600 vfx::INVALID: Invalid VFX operation
owner: ara-vfx

`ara-vfx` reports invalid effects, graph projections, bindings or simulation inputs. Invalid authoring commands are rejected and leave the effect unchanged.

Causes:
- VFX names repeat, modules occupy the wrong stages, an emitter lacks exactly one lifetime and one geometry module, or parameter bindings have the wrong scope or type.
- A VFX command targets a missing emitter/module/parameter, CPU depth collision lacks an authoritative `DepthQuery`, or the simulation tick counter is exhausted.

Fixes:
- Correct the effect with `vfx.text.apply` or `vfx.stack.edit`; give modules unique names, legal stages and valid typed bindings.
- Supply an authoritative `DepthQuery` for CPU depth collision, or reset the VFX instance if its tick counter is exhausted.

## ARA3601 vfx::QUALITY: VFX quality limit reached
owner: ara-vfx

Quality limits reduce particle or light work. VFX continues with particle capacity allocated in authored emitter order and later excess lights omitted.

Causes:
- Emitter capacities exceed `quality.particles`, or render light modules exceed `quality.lights`.

Fixes:
- Lower emitter capacities or light counts, reorder emitters to prioritize their allocations, or raise the corresponding valid quality limit with `vfx.text.apply`.

## ARA3602 physics2d::COMMAND: Invalid 2D physics command
owner: ara-physics2d

A `physics2d.set` or `tilemap.colliders.generate` request cannot produce valid physics authoring data. The command transaction is rejected, including any staged replacement of generated colliders.

Causes:
- The request does not provide exactly `body` plus `collider`, or exactly `character`.
- Pose, mass, collider geometry/material/layer or capsule controller settings fail validation.
- Collider generation targets a prefab address or missing tilemap, or supplies a nonfinite origin or singular basis.

Fixes:
- Resubmit `physics2d.set` with one valid role: `body` and `collider`, or `character`, plus a finite pose.
- Correct the reported dimensions and material values; polygon colliders need `3..64` finite points forming a nondegenerate hull, and character height must exceed twice its positive radius.
- Run `tilemap.colliders.generate` on a plain tilemap entity with a finite origin and invertible basis; generate prefab colliders in the prefab's source document.

## ARA3610 terrain::INVALID: Invalid terrain operation
owner: ara-terrain

`ara-terrain` reports invalid terrain data, authoring operations or tile cooking. Invalid authoring commands are rejected; a failed cook retains the last good tile rather than publishing the failed result.

Causes:
- Terrain grid, layer, brush or tile data is invalid, a referenced payload is missing or corrupt, or a cook cannot run or exceeds its dependency/residency bounds.

Fixes:
- Correct the named grid, layer or brush fields through terrain commands; use power-of-two tile cell counts from `16` through `256` and layer opacity in `0..1`.
- Split gestures touching more than `4096` tiles, reduce erosion stacks exceeding the `256`-sample dependency halo, or increase residency capacity when a tile exceeds its budget.
- Restore the named terrain payload or worker executable, use a fresh unique job output path, then retry the cook.

## ARA3620 2d::COMMAND: 2D command rejected
owner: ara-2d

`ara-2d` rejected an authoring command. Failed strokes and setters leave their transaction unapplied.

Causes:
- A command targets an entity without `Tilemap`, an unknown layer or invalid tile data.
- A fill has invalid bounds, excludes its seed or exceeds `65536` cells; a stroke exceeds its sample limit, or a component setter receives invalid geometry, light, camera or flipbook values.

Fixes:
- Target an existing tilemap layer and correct tile definitions with `tilemap.set` before retrying `tilemap.paint` or `tilemap.rule.apply`.
- Keep `tilemap.fill` bounds within `65536` cells and include the seed; split oversized strokes and correct the component values named in the error before retrying the setter.

## ARA7001 physics::BACKEND: Physics operation failed
owner: ara-physics

A `PhysicsError` has been converted to a structured diagnostic. The requested physics operation failed without editing authoring documents. An ECS boundary failure is sticky in `PhysicsStatus`; it is not a guarantee that earlier runtime work in that boundary was rolled back.

Causes:
- Physics configuration, shapes or body roles are invalid, such as a nonpositive fixed interval or a `RigidBody` without `Collider`.
- `PhysicsId` is zero, duplicated or missing for a body intent or joint connection.
- A snapshot's topology, fixed interval or backend configuration differs from the target world, or the backend rejects a requested feature or native operation.

Fixes:
- Correct the condition named in the message: use a finite positive fixed interval, valid body/collider roles and unique nonzero `PhysicsId` values.
- Resolve runtime intents and joint connections to existing bodies by stable identity.
- Before restoring, reconstruct the checkpoint's matching world topology and configuration; inspect the session's `PhysicsStatus` before allowing simulation to continue.

## ARA7100 nav::BUILD: Navigation reachability result
owner: ara-nav

`why_unreachable` reports evidence about a route on the current baked mesh; despite this catalog name, the code is not a generic bake failure. A complete permitted corridor is informational, while an unreachable or capacity-limited query is a warning. The query does not change the mesh and may return the nearest reachable point.

Causes:
- The start has no permitted nearby polygon, or the goal is outside the baked surface.
- The connecting corridor or goal uses a forbidden area, or disconnected islands need a link or geometry change.
- Path search exhausts its capacity; a disabled authored link may be suggested as a candidate, not proof of a connection.
- A complete permitted corridor exists, producing an informational result that needs no repair.

Fixes:
- Move or project endpoints onto permitted navigation polygons, or rebake the relevant geometry when the intended surface is absent.
- Inspect the reported area filter and candidate link; enable the intended link or add the missing connection, then query again.
- Increase query capacity before concluding that a truncated search proves disconnection.
- Leave a successful informational result unchanged.

## ARA7300 play::PLAY: Player call failed
owner: ara-play-host

A request to a play-mode `ara-player` process failed: the link timed out, the player rejected the call, or a reply could not be decoded. The editor and its documents are unaffected; the world keeps its last good state.

Causes:
- The player is busy past the request timeout (a long step or a blocking load).
- The player binary is older or newer than the host and speaks a different play protocol.
- The shared-memory or JSON-RPC link was closed mid-call.

Fixes:
- Retry the call; use `ara_play` with `action: status` to see whether the world is still alive.
- Rebuild `ara-player` together with the editor so both use the same protocol.
- Stop the world and start a new one from the pool.

## ARA7301 play::PLAYER_FAULT: Player crashed or hung
owner: ara-play-host

The player process for a world panicked, crashed, hung or exited. Play runs out of process (S5), so the editor kept running; the host restarts players from the pool and reports the fault here.

Causes:
- A gameplay system or script panicked (the message carries the panic text when available).
- Native code faulted (access violation) in a plugin or driver.
- A system looped without yielding and the hang watchdog fired.

Fixes:
- Read the panic message and `ara why` evidence; fix the failing system, then press Play again.
- Run the scene headless with `ara_play` and `ara_test` to reproduce deterministically from the recorded seed and inputs.
- Check driver and validation diagnostics (`ARA_GPU_*`) for native faults.

## ARA7302 play::UNKNOWN_WORLD: Unknown play world
owner: ara-play-host

A play call named a world ID that the play service does not know. Worlds end when they are stopped or their player exits.

Causes:
- The world was stopped, or its player crashed and was not restarted.
- The ID belongs to another editor session or daemon.

Fixes:
- List live worlds with `ara_play` (`action: status`) and use one of their IDs, or omit `world` to target the latest world.
- Start a new world with `play.start`.

## ARA7303 play::ASSETS: Play assets not ready
owner: ara-play-host

The scene references assets that are not imported yet or failed to import, so the player cannot load them.

Causes:
- An import is still running for a referenced asset.
- An import failed (see the related `ARA21xx` asset diagnostic).
- A referenced asset's source is missing.

Fixes:
- Wait for `jobs` to finish, then press Play again.
- Fix the failing import named in the message and re-import it.
- Point the referencing field at an asset that exists (`property.set`).

## ARA7304 play::TEST_FAILED: Playtest assertion failed
owner: ara-play-host

An `ara_test` playtest ran and one of its assertions did not hold. The test report lists the failing assertion, the tick and the observed values.

Causes:
- Gameplay behaviour changed (intended or a regression).
- The test's expectation or timing window is wrong for the scene.

Fixes:
- Re-run the test and inspect its captures and the failing tick's state with `ara_inspect`.
- Fix the regression, or update the assertion if the new behaviour is intended.

## ARA7305 play::REQUEST: Invalid play request
owner: ara-play-host

The play request itself is invalid: an unknown scene, an unsupported mode, or a malformed spec (player count, window options, ticks).

Causes:
- The scene path or ID does not exist.
- The mode or player configuration is not supported by this build.
- A parameter has the wrong type or range.

Fixes:
- Use `ara_query` to find the scene, then pass its path or ID.
- Check the `ara_play` tool description for the supported modes and parameters, and resend.

## ARA7401 l10n::MISSING_KEY: Missing localization key
owner: ara-l10n

A locale lacks a source key, or lookup cannot find a translation in any fallback. Parity checking emits a warning without changing the table; lookup returns an error only after its locale, language, configured and source fallbacks are exhausted.

Causes:
- A source key was added without a corresponding target translation.
- A lookup uses an absent key, or none of the loaded fallback tables contains it.

Fixes:
- Apply the parity diagnostic's `l10n.put` fix to insert source text marked `NeedsReview`, then translate and review it.
- Correct the lookup key or load the intended source and fallback tables containing that key.

## ARA7402 l10n::EXTRA_KEY: Obsolete localization key
owner: ara-l10n

A target locale contains a key absent from the source table. This parity warning does not remove the entry or block publication by itself.

Causes:
- A source key was removed or renamed while its old translation remained.
- The target table contains a key that was never added to the source locale.

Fixes:
- Apply the attached `l10n.remove` command for the obsolete message record.
- If the key is intentional, add its source entry with `l10n.put` so source and target tables agree.

## ARA7403 l10n::MESSAGE: Invalid localization data
owner: ara-l10n

A localization operation encountered invalid message syntax, signatures, arguments or supporting table/exchange data. The operation fails; invalid catalog edits are not published over the last good live catalog. A formatting error returns no formatted message.

Causes:
- An ICU pattern has unmatched braces, duplicate selectors, no `other` branch or undeclared, unused or mistyped parameters.
- Runtime arguments do not match the signature, locale tables disagree on parameter types, or required source tables are absent.
- Table records or keys are invalid, or an XLIFF/PO import has stale source text, unknown keys or unsupported directives instead of whole ICU patterns.

Fixes:
- Correct the reported message syntax and make its declared parameters match every usage; include an `other` branch for each plural/select.
- Supply the exact typed arguments and load source/target tables with matching signatures; submit table repairs through `l10n.put`.
- Re-export stale translation data, preserve whole ICU patterns and metadata, then import against the current source.

## ARA7404 l10n::TABLE: Invalid locale identifier
owner: ara-l10n

A locale identifier could not be parsed by ICU's locale parser. The table or locale-dependent operation is rejected before a normalized locale can be used; this code currently identifies locale parsing failures specifically.

Causes:
- A table locale, source locale, fallback or requested locale contains malformed locale syntax.

Fixes:
- Replace the value named in the message with a valid locale identifier such as `en`, `en-US` or `ar`, then reload the table or retry the request.

## ARA7405 l10n::TRANSLATION: Translation response rejected
owner: ara-l10n

A generated translation response does not match its request or has become stale. Provider validation rejects mismatched responses; `l10n.translate_job` rejects stale application atomically, preserving current translations and reviewed text.

Causes:
- Job identity is empty or mismatched, provider/model provenance is missing, or the response translation count differs from the request.
- Source text, notes, parameters or source/target fingerprints changed after generation began.
- The target entry has already been marked `Reviewed`.

Fixes:
- Prepare a fresh translation request from the current source and target, run the provider again and submit the matching response through `l10n.translate_job`.
- Ensure the provider returns the same job ID, all requested keys and nonempty provider/model provenance.
- Keep reviewed entries out of machine translation jobs; make intentional reviewed-text changes through `l10n.put`.

## ARA7501 save::CORRUPT: Invalid save data
owner: ara-save

Save bytes fail envelope, checksum, size, decompression, CBOR or state-hash checks. Encoding also uses this code when a save exceeds configured limits. Invalid bytes are not restored; `SlotStore::load` tries a verified backup after a primary corruption or I/O failure and fails if neither copy is usable.

Causes:
- The file is truncated, altered, has an unsupported envelope version/codec, or does not match its declared length or checksum.
- Decoded data has duplicate keys, nonfinite numbers, excessive nesting, trailing bytes or an incorrect persistent-state hash.
- The slot metadata disagrees with its filename, or encoded/decoded size exceeds configured limits.

Fixes:
- Load the slot through `save.load` to attempt verified backup recovery; if recovery succeeds, use `save.restore_backup` to repair the primary.
- If both copies fail, restore a known-good copy with matching slot metadata rather than editing checksums or binary payloads.
- For a size-limit error on otherwise valid state, reduce the captured persistent data or deliberately adjust the host's `Limits` before retrying.

## ARA7502 save::SCHEMA: Save schema mismatch
owner: ara-save

Persistent state does not match the registered save policy, schemas or codecs. Capture, migration or restore fails; schema incompatibility does not trigger automatic backup fallback. If the message reports a restored-state hash mismatch, restoration has already run and verification failed afterward.

Causes:
- Persistent types are unknown, duplicated, registered with the wrong storage class, or lack the required simulation codecs.
- Saved type sets differ, versions are zero or from the future, or a required migration cannot complete.
- State contains invalid identities, hierarchy cycles, missing parents, excluded data or references into missing/excluded state.

Fixes:
- Register matching reflected types and component/resource codecs in `SaveSchema` and `SimRegistry` before capture or restore.
- Provide explicit policy migration for changes to the persistent type set and schema upgrade hooks for supported older field versions; use a compatible engine for future versions.
- Repair the persistent hierarchy and references, retaining a selected component on referenced entities.
- For a post-restore hash mismatch, inspect the restored session and codec behavior before resuming play; do not assume the prior state was retained.

## ARA7503 save::AUTHORITY: Save authority required
owner: ara-save

The caller is a replica client, or the host's apply policy has deferred a load. The write, backup repair or authoritative-state application is refused before it proceeds.

Causes:
- A client attempts to capture/write, repair or restore authoritative save state.
- `apply_load` is called when the host has not allowed application at that simulation boundary.

Fixes:
- Send the save intent to the authoritative server, or perform the operation in a local session with trusted host authority.
- For a deferred load, retain the prepared save and retry application at a boundary permitted by the host's apply policy.

## ARA7504 save::IO: Save storage operation failed
owner: ara-save

A slot operation encountered an I/O error or rejected its slot name, conflict token or concurrent writer. Writes use synced temporary files and atomic replacement, so a failure is not an invitation to overwrite the slot blindly. A primary load I/O failure may instead recover from a verified backup.

Causes:
- Save files cannot be opened, read, compressed, written, synced or atomically replaced.
- Another writer holds the slot lock, or the expected file hash no longer matches the current slot.
- The slot name is not portable, or an expected hash is not exactly `64` hexadecimal digits.

Fixes:
- Correct the reported filesystem problem, including directory access or available disk space, then retry the operation.
- Let the active writer finish; reload and compare the local/cloud versions before writing with a refreshed expected file hash.
- Use a portable slot name such as `autosave_1` and pass the complete file hash returned by the previous read.

## ARA7505 save::BACKUP: Save recovered from backup
owner: ara-save

The primary slot failed to load, but its verified backup succeeded. This warning accompanies the recovered save; loading does not rewrite the primary file automatically.

Causes:
- The primary is missing, unreadable or corrupt while the previous-generation backup remains valid.

Fixes:
- Apply the attached `save.restore_backup` action for the slot to restore the verified backup atomically, using local or server authority.
- Investigate the primary file failure before the next save; the recovered state may be one generation older than the failed primary.
