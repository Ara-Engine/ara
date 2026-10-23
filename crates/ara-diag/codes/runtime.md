# Assets, world streaming, networking and render visibility (ARA2100–ARA2499, ARA3300–ARA3399)

Runtime problems reported through the bus by `ara-asset`, `ara-world` and
`ara-net`, and the render-visibility causes `ara why` reports (the renderer can
report the same codes at runtime).

## ARA2101 ASSET_IMPORT_FAILED: Asset import failed
owner: ara-asset

An importer could not turn the source file into engine data. The asset keeps its last good imported version; entities using an asset that never imported have nothing to draw or play.

Causes:
- The source file is corrupt, truncated or in an unsupported variant of its format (a glTF without meshes, an unsupported texture compression).
- The import settings in the `.import.json` sidecar are invalid for this source.
- The source was being written by another program when it was read.

Fixes:
- Read the importer's message; re-export the source from its authoring tool.
- Fix or reset the sidecar settings, then re-import (`jobs.start` with the import job).
- Save the file again once the other program has finished writing it.

## ARA2102 ASSET_SOURCE_MISSING: Asset source missing
owner: ara-asset

The asset index knows the asset (its `.import.json` sidecar exists, so references resolve and `ara check` passes) but the source media next to the sidecar is missing. Nothing can be imported; renderers fall back to a placeholder or draw nothing.

Causes:
- The source file was not committed (Git LFS not pulled, or ignored by `.gitignore`).
- The source was renamed or moved without its sidecar.

Fixes:
- Restore the file (`git lfs pull`, or copy it back next to its sidecar).
- Move the sidecar along with a renamed source so the asset keeps its ID.
- Point the referencing fields at an asset that exists (`property.set`).

## ARA2103 ASSET_WORKER_FAILED: Import worker failed
owner: ara-asset

An import worker process crashed, timed out or broke the worker protocol. Importers run out of process (A13), so the editor kept running; the import did not finish.

Causes:
- A bug or a crash in an importer for this particular source.
- The worker binary is missing or from another engine version.
- The machine ran out of memory during a large import.

Fixes:
- Retry the import; if it fails again, report the source file with the worker's message.
- Rebuild the engine so the worker matches the host.

## ARA2104 ASSET_IMPORT_CANCELLED: Asset import cancelled
owner: ara-asset

An import was cancelled before it was published, usually because a newer edit of the same source superseded it. The newer import replaces it.

Causes:
- The source changed again while it was importing.
- Someone cancelled the job.

Fixes:
- Nothing to fix; wait for the newer import, or start the job again if it was cancelled by hand.

## ARA2201 WORLD_INVALID: Invalid world data
owner: ara-world

World authoring data, a cooked cell or the streaming configuration is invalid. The affected world or cell is not loaded.

Causes:
- A `.world` manifest names a cell or root scene that does not exist.
- A cooked cell blob is from another build (checksum or schema generation mismatch).
- Streaming limits that cannot work (zero budgets, a cell bigger than the memory admission limit).

Fixes:
- Run `ara check` on the world and cell documents.
- Re-cook the world's cells after schema changes.
- Raise the streaming budget or split the oversized cell.

## ARA2202 WORLD_CELL_FAILED: World cell failed to load
owner: ara-world

A streaming cell failed to load (IO, decode or activation). It stays unloaded and needs an explicit retry; entities in it are neither simulated nor drawn.

Causes:
- The cell's cooked data is missing or corrupt.
- A materialized row is over the 16 KiB limit (bulky data belongs in assets).
- The memory reservation could not be admitted.

Fixes:
- Re-cook the cell, then retry it.
- Move bulky component data into an asset and reference it.

## ARA2203 WORLD_NOT_RESIDENT: World cell not resident
owner: ara-world

The entity's world cell is streamed out: no interest source (camera, player) is close enough, so the entity is neither simulated nor drawn right now. This is normal streaming behaviour, not an error.

Causes:
- The camera or player is far from the cell.
- The cell's layer is not active.
- The cell is still loading.

Fixes:
- Move the camera or player near the entity, or raise the streaming radius.
- Activate the cell's layer.

## ARA2401 NET_REJECTED: Network operation rejected
owner: ara-net

A network configuration, packet or session operation was rejected. Networking keeps running; the rejected operation had no effect.

Causes:
- Invalid network conditions (simulated loss or latency out of range).
- A client intent outside its tick or size window, or a replayed sequence.
- An expose policy that is invalid or declared twice.

Fixes:
- Read the message for the rejected field; correct the configuration or the intent.
- For replayed or late intents, check the client's clock and tick alignment.

## ARA3301 RENDER_NO_RENDERER: Nothing to draw
owner: ara-render

The entity has no renderer component (`MeshRenderer`), so it has nothing to draw. Its children may still draw.

Causes:
- The renderer was never added, or was removed.
- The mesh is meant to come from a prefab, but the entity is not an instance of it.

Fixes:
- Add one: `component.add` with `MeshRenderer` and a `mesh` (`ara_apply`).
- Make the entity an instance of the prefab that carries the renderer.

## ARA3302 RENDER_DISABLED: Disabled or hidden
owner: ara-render

The entity, one of its ancestors, or its renderer is disabled or hidden (`enabled: false`, `visible: false`, `hidden: true`), so it is skipped.

Causes:
- A flag turned off while testing and never turned back on.
- A parent group was hidden, which hides everything under it.

Fixes:
- Set the flag back with `property.set` on the entity or ancestor that has it.

## ARA3303 RENDER_DEGENERATE_TRANSFORM: Zero scale
owner: ara-render

The entity's world scale is zero on at least one axis, from its own `Transform.scale` or an ancestor's. A mesh flattened to zero thickness covers no pixels.

Causes:
- A scale component set to 0 (often by an animation or a typo like `[1, 0, 1]`).
- A parent group scaled to 0 to hide its children.

Fixes:
- Set the scale back on the entity or ancestor that has the zero (`property.set` `Transform.scale` `[1, 1, 1]`).
- Hide groups with a disabled flag instead of a zero scale.

## ARA3304 RENDER_NON_FINITE_TRANSFORM: NaN or infinite transform
owner: ara-render

A position, rotation or scale of the entity or an ancestor is not a finite number (NaN or infinity), so the whole subtree has no valid world position and is not drawn. In documents it shows as `null` in a vector.

Causes:
- A physics or gameplay system divided by zero or normalized a zero vector, and the result was written back.
- A broken import or a script wrote NaN into a transform.

Fixes:
- Reset the transform with `property.set` (for example `Transform.position` `[0, 0, 0]`).
- Find the system that wrote it: `ara logs --code ARA3304` shows when it first happened, with the tick.

## ARA3305 RENDER_NO_MESH: No mesh to draw
owner: ara-render

The renderer names no mesh, or its mesh reference does not resolve to an asset, or the asset's source file is missing, or its import failed. There is no geometry to draw.

Causes:
- The `mesh` field is empty.
- The mesh asset was renamed or deleted (ARA1302), or its source is missing (ARA2102).
- The mesh failed to import (ARA2101).

Fixes:
- Point `MeshRenderer.mesh` at an existing mesh with `property.set` (the diagnostic suggests the closest match).
- Restore the source file or fix the import (see the linked diagnostic).

## ARA3306 RENDER_MATERIAL: Material unusable
owner: ara-render, ara-material

The renderer's material does not resolve to an asset, its file is missing, or it does not compile (ARA3200–ARA3299 diagnostics carry the line and column). Without a compiled material there is no shader to draw the mesh with.

Causes:
- A syntax or type error in the `.material` file.
- The material reference is stale after a rename.
- The material uses a domain that cannot draw meshes (UI, post-process).

Fixes:
- Fix the error at the reported line in the `.material` file; `ara why @materials/<name>.material` lists the compile diagnostics.
- Point the renderer at a working material with `property.set`.

## ARA3307 RENDER_TRANSPARENT: Fully transparent
owner: ara-render, ara-material

The material's opacity is zero, or the renderer's tint alpha is zero, so every pixel the mesh covers is blended away.

Causes:
- A tint color with alpha 0 (`[1, 1, 1, 0]`).
- A translucent material whose `opacity` output is a constant 0.

Fixes:
- Set the tint alpha to 1 (`property.set` `MeshRenderer.tint` `[1, 1, 1, 1]`).
- Give the material a non-zero opacity, or make it opaque.

## ARA3308 RENDER_LAYER_CULLED: Culled by camera mask
owner: ara-render

The entity's render layer is not in the culling mask of any camera that renders the scene, so every camera skips it.

Causes:
- The entity was moved to a layer (debug, editor-only, UI) that game cameras do not render.
- A camera's culling mask was narrowed and no longer includes the layer.

Fixes:
- Put the entity back on a rendered layer (`property.set` on its `layer` field).
- Add the layer to the camera's `cullingMask`.

## ARA3309 RENDER_DISTANCE_CULLED: Beyond draw distance
owner: ara-render

The entity is farther from every camera than the camera's far plane, or than its own draw distance (a renderer `maxDistance` or LOD cull distance), so it is culled.

Causes:
- The entity sits far from the play area (a misplaced position, often a unit mix-up: centimeters typed as meters).
- A small `far` on the camera or a small draw distance on the renderer.

Fixes:
- Move the entity into range (`property.set` `Transform.position`).
- Raise the camera's `far` or the renderer's draw distance.

## ARA3310 RENDER_NO_CAMERA: No camera
owner: ara-render

The scene has no enabled camera, so nothing in it is drawn at runtime (the editor viewport has its own camera, so things may still show there).

Causes:
- The camera entity was deleted or disabled.
- The camera lives in another scene that is not loaded.

Fixes:
- Add a camera: `entity.create` with a `Camera` component.
- Enable the existing camera.

## ARA3311 RENDER_PASS_EXCLUDED: Excluded from every pass
owner: ara-render

The renderer left the entity out of every render pass it ran (reported by the running renderer with the passes as subjects).

Causes:
- The material's domain or blend mode is not handled by any active pass.
- A pass filter (shadow-only, editor-only) matches the entity.

Fixes:
- Use a material whose domain the active passes draw (a `surface` material for meshes).
- Check the pass filters named in the diagnostic.

## ARA3312 RENDER_OUT_OF_VIEW: Behind every camera
owner: ara-render

The entity is behind every camera (outside the view frustum), so no camera sees it from where it stands now.

Causes:
- The entity was placed on the wrong side of the camera.
- The camera faces another way than intended.

Fixes:
- Move the entity in front of the camera, or turn the camera toward it.
- This may be expected: a moving camera can see it later.
