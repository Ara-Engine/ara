# Reflection probes, debug views and the Renderlab bridge (named codes, slots 6100–6199)

`ara-runtime-render` reports reflection-probe capture jobs and `ara-render` debug view
availability with named codes. Capture jobs are derived runtime work: they never edit
documents, and a failed or cancelled capture always keeps the last valid one.
`ara-renderlab` reports what the Renderlab renderer path cannot start, draw or fit
(6120–6123).

## ARA6101 PROBE_MISSING: Reflection probe not found
owner: ara-runtime-render
shown as: ARA_PROBE_MISSING

A recapture request names a probe that is not a live `ReflectionProbe` entity of the scene the renderer currently holds.

Causes:
- The entity was deleted, or its `ReflectionProbe` component removed, before the request.
- The request's document or entity address belongs to another scene than the one rendered.

Fixes:
- Query `render.probes.query` for the probes of the current scene and request those.
- Wait for the edit to reach the renderer, then request again.

## ARA6102 PROBE_INVALID: Reflection probe cannot be captured
owner: ara-runtime-render
shown as: ARA_PROBE_INVALID

The probe's settings or world transform are outside what a capture accepts.

Causes:
- `bounds`, `projectionBounds`, `blendDistance`, `priority` or `intensity` is nonfinite or out of its range.
- The entity's world transform is singular (a zero scale) or nonfinite.

Fixes:
- Correct the field named in the message within its documented range.
- Give the entity and its parents a nonzero, finite scale.

## ARA6103 PROBE_RESOLUTION: Unsupported capture resolution
owner: ara-runtime-render
shown as: ARA_PROBE_RESOLUTION

`captureResolution` is not one of 64, 128, 256, 512 or 1024 texels per face. The request is rejected before anything is allocated.

Causes:
- The value was typed or scripted instead of chosen from the supported sizes.

Fixes:
- Set `captureResolution` to one of `ReflectionProbe::RESOLUTIONS`.

## ARA6104 PROBE_LIMIT: Too many or too large reflection probes
owner: ara-runtime-render
shown as: ARA_PROBE_LIMIT

The renderer holds at most 16 probes, and all of their captures together must fit one capture atlas.

Causes:
- More enabled probes than the renderer holds in the scene.
- Many probes at 1024 texels per face exceed the atlas.

Fixes:
- Disable or remove probes that overlap little, or merge them into one larger box.
- Lower `captureResolution` on probes that serve rough or distant surfaces.

## ARA6105 PROBE_UNSUPPORTED: Reflection probes not shaded here
owner: ara-runtime-render
shown as: ARA_PROBE_UNSUPPORTED

The current quality preset or device does not shade reflection probes, so a capture would not change what is drawn.

Causes:
- The preset is Low or Medium: probes shade at High and Epic only.
- The renderer is not the native raster renderer.

Fixes:
- Switch the view to the High or Epic preset, then request the capture.

## ARA6106 PROBE_STALE: Capture discarded as stale
owner: ara-runtime-render
shown as: ARA_PROBE_STALE

The probe's inputs (its settings, the scene's geometry, materials, lighting or environment) changed, or the probe disappeared, while its capture was in flight. The replacement was discarded and the last valid capture kept.

Causes:
- An edit landed during the capture.
- The probe was deleted or disabled during the capture.

Fixes:
- Request the capture again once editing settles; repeated requests coalesce.

## ARA6107 PROBE_CACHE: Probe cache read or write failed
owner: ara-runtime-render
shown as: ARA_PROBE_CACHE

A completed capture could not be written under `.ara/cache/probes`, or a cached capture could not be read back. The last valid capture stays in use; a missing cache entry is captured again.

Causes:
- The project's `.ara/cache` folder is read-only or the disk is full.
- A cache file was truncated or edited outside the engine.

Fixes:
- Free disk space or fix the folder's permissions, then recapture.
- Delete the probe's cache files; they are derived and are captured again.

## ARA6108 PROBE_CANCELLED: Capture cancelled
owner: ara-runtime-render
shown as: ARA_PROBE_CANCELLED

The capture was cancelled before it published. The last valid capture is kept.

Causes:
- `render.probe.cancel` was requested for the job.
- A newer request for the same probe superseded it.

Fixes:
- Request the capture again when needed.

## ARA6109 VIEW_MODE_UNAVAILABLE: Debug view unavailable
owner: ara-render
shown as: ARA_VIEW_MODE_UNAVAILABLE

The requested debug view is not supported or not ready on this renderer, so the view keeps its previous mode instead of drawing Lit under the debug view's name.

Causes:
- The renderer path or quality preset does not produce the view (for example GI diffuse without probe GI).
- The view's data is not ready yet (probe GI still converging).

Fixes:
- Follow the reason in `render.probes.query` capabilities; switch the preset or wait for readiness.

## ARA6120 RENDERLAB_UNAVAILABLE: Renderlab renderer could not start
owner: ara-renderlab
shown as: ARA_RENDERLAB_UNAVAILABLE

The launch uses the Renderlab renderer (the default, or `--renderer renderlab*` or `ARA_RENDERER`), but it could not start on this machine. The view renders with Ara's own renderer (`ara-render`, frozen) instead, which is complete on every device, and the player says so once (`Ara presentation: ara-render draws (...)`).

Causes:
- No Vulkan adapter, or the GPU lacks the features the requested Renderlab path needs (hardware ray queries for `renderlab-rt`; 64-bit storage atomics and descriptor indexing for `renderlab-nonrt`).
- The machine is a Mac: Renderlab renders through Vulkan only and has no Metal path yet.
- Renderlab's engine failed to create its device or pipelines; the message carries its error.

Fixes:
- Use `--renderer renderlab` to let it pick the path the GPU supports, or `--renderer ara`.
- Update the GPU driver, then run `ara doctor`.

## ARA6121 RENDERLAB_UNSUPPORTED: Feature not rendered by Renderlab
owner: ara-renderlab
shown as: ARA_RENDERLAB_UNSUPPORTED

A frame uses something Ara's renderer draws that the Renderlab path does not map yet. The message names the feature and what the image shows instead. Each feature reports once per renderer; the rest of the frame renders normally.

Causes:
- The scene uses a feature on the bridge's gap list (`crates/ara-renderlab/AGENTS.md`): for example terrain and foliage, VFX graph effects, decals, emissive materials, environment-map skies or area lights.

Fixes:
- Nothing is required to keep working: switch to `--renderer ara` to see the feature.
- To close the gap, map the feature in `ara-renderlab` (its gap list says what each one takes).

## ARA6122 RENDERLAB_LIMIT: Scene exceeds a Renderlab limit
owner: ara-renderlab
shown as: ARA_RENDERLAB_LIMIT

The frame needs more than Renderlab draws at this version, so part of it is drawn in a cheaper form or left out. The message names the limit and what happened.

Causes:
- More than 256 distinct mesh-material groups in one frame (objects sharing a mesh and a material count once; every skinned primitive counts once). Opaque static groups that share a material are then merged into one world-space batch per material; a batch whose members move is rebuilt each frame.
- More distinct materials than the limit even after merging: the last-appearing draws are left out.
- An effect emitter needs more live particles than Renderlab's emitters hold.

Fixes:
- Share materials between static objects: merged batches then fit, and nothing is left out.
- Share meshes and materials between objects, or render the scene with `--renderer ara`.
- The limits are Renderlab's (`TRACED_DRAWS`, `MAX_PARTICLES`); raising them is a renderer change, not a bridge change.

## ARA6123 RENDERLAB_NONRT_PATENT: Renderlab non-RT sun shadows await legal review
owner: ara-renderlab
shown as: ARA_RENDERLAB_NONRT_PATENT

A notice, printed once when Renderlab's non-RT path starts. Its sun-shadow method carries an open patent question (Renderlab `docs/patent-via-8471853.md`: an engineering claim chart, not legal clearance). The image is correct; the notice is about shipping.

Causes:
- `--renderer renderlab-nonrt` was selected, or `--renderer renderlab` chose the non-RT path because the GPU has no hardware ray queries.

Fixes:
- Prefer `--renderer renderlab-rt` on GPUs with hardware ray tracing: that path does not use the method.
- Do not ship the non-RT path in a product before a legal review, or before Renderlab replaces the method.
