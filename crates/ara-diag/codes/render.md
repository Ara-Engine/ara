# Reflection probes and debug views (named codes, slots 6100–6199)

`ara-runtime-render` reports reflection-probe capture jobs and `ara-render` debug view
availability with named codes. Capture jobs are derived runtime work: they never edit
documents, and a failed or cancelled capture always keeps the last valid one.

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
