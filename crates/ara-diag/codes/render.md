# Debug views and the Renderlab bridge (named codes, slots 6100–6199)

`ara-runtime-render` reports debug view availability with a named code.
`ara-renderlab` reports what the Renderlab renderer path cannot start, draw or fit
(6120–6123).

## ARA6109 VIEW_MODE_UNAVAILABLE: Debug view unavailable
owner: ara-runtime-render
shown as: ARA_VIEW_MODE_UNAVAILABLE

The requested debug view is not supported on this renderer, so nothing is drawn under the debug view's name (Renderlab draws the Lit view only).

Causes:
- A capture asked for a view other than Lit (`ara capture --view`, a labelled capture's mode).

Fixes:
- Drop the view option to capture the lit image.

## ARA6120 RENDERLAB_UNAVAILABLE: Renderlab renderer could not start
owner: ara-renderlab
shown as: ARA_RENDERLAB_UNAVAILABLE

The launch uses Renderlab (the only renderer, or an explicit `--renderer renderlab*` / `ARA_RENDERER` path), but it could not start on this machine. The player stops with this error. There is no other renderer to draw the view.

Causes:
- No Vulkan adapter, or the GPU lacks the features the requested Renderlab path needs (hardware ray queries for `renderlab-rt`; 64-bit storage atomics and descriptor indexing for `renderlab-nonrt`).
- On a Mac, Renderlab's Metal backend (the presenter's device) failed to start.
- Renderlab's engine failed to create its device or pipelines; the message carries its error.

Fixes:
- Use `--renderer renderlab` so the launch picks the path the GPU supports, or name `renderlab-rt` / `renderlab-nonrt` only when that path can start.
- Update the GPU driver, then run `ara doctor`.

## ARA6121 RENDERLAB_UNSUPPORTED: Feature not rendered by Renderlab
owner: ara-renderlab
shown as: ARA_RENDERLAB_UNSUPPORTED

A frame uses something Ara's authoring supports that the Renderlab path does not map yet. The message names the feature and what the image shows instead. Each feature reports once per renderer; the rest of the frame renders normally.

Causes:
- The scene uses a feature on the bridge's gap list (`crates/ara-renderlab/AGENTS.md`): for example terrain and foliage, VFX graph effects, decals, emissive materials, environment-map skies or area lights.

Fixes:
- The feature stays a gap until Renderlab maps it (`crates/ara-renderlab/AGENTS.md`).
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
- Share meshes and materials between objects so the frame fits Renderlab's limit.
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
