# Ara VFX

Effects are systems of named emitters with four ordered module stacks:
`spawn → initialize → update → render`. Every module has a documented Rust type
and a matching CPU implementation and generated Slang implementation.

`.vfx` is strict JSON without asset, node or edge IDs. It is a text view of an
`ara-graph` document, not a second authoring authority. `Effect::parse` rejects
duplicate keys, unknown fields, invalid numeric inputs and modules in the wrong
stack. `Effect::text` uses ara-json's canonical writer. Complete examples are in
[`examples/`](examples/).

```json
{
  "name": "embers",
  "seed": 17,
  "hz": 60,
  "emitters": [{
    "name": "embers",
    "capacity": 1024,
    "bounds": {"min": [-4, -1, -4], "max": [4, 8, 4]},
    "stacks": {
      "spawn": [{"name": "rate", "module": {"op": "rate", "per_second": 60}}],
      "initialize": [
        {"name": "life", "module": {"op": "lifetime", "seconds": 2, "size": 0.04, "color": [5, 1, 0.1, 0.8]}},
        {"name": "rise", "module": {"op": "velocity", "value": [0, 1, 0], "spread": [0.2, 0.1, 0.2]}}
      ],
      "update": [{"name": "wind", "module": {"op": "curl_noise", "frequency": 2, "strength": 0.5}}],
      "render": [{"name": "sprite", "module": {"op": "sprite", "domain": "unlit", "soft_distance": 0.1, "sorting": "none"}}]
    }
  }]
}
```

Every emitter needs one lifetime module and one geometry output. Missing stacks
are empty. Quality defaults are 1M particles, 16 estimated full-screen layers,
eight emitter lights and 4096 event children per emitter/tick. Allocations use
authored emitter order. Quality reductions return ARA3601 diagnostics and counts.

Parameters have `namespace`, `name`, `doc` and a typed `default`, for example:
`{"namespace":"user","name":"wind","doc":"Wind acceleration.","default":{"type":"float","value":2}}`.
Bind a module field with `"bindings":{"strength":"user.wind"}` alongside its
`module`. Namespaces are `system`, `emitter`, `particle` and `user`; an emitter
parameter is named `emitterName.key`. Particle parameters are immutable spawn
defaults, while mutable position/velocity/color/age live in SoA channels.
`Effect::resolve` checks overrides and binding shapes before execution/cooking.

| Stack | Modules |
|---|---|
| Spawn | `rate`, `burst` |
| Initialize | `lifetime`, `point`, `sphere`, `box`, `cone`, `mesh_surface`, `velocity` |
| Update | `gravity`, `drag`, `curl_noise`, `vortex`, `attractor`, `color_over_life`, `size_over_life`, `plane_collision`, `depth_collision` |
| Render | `sprite`, `flipbook`, `ribbon`, `mesh`, `light`, `event_spawn` |

Mesh surfaces sample triangles by area; asset workers provide world-space cooked
triangles. Curves/gradients have 2–16 strictly ordered normalized lifetime keys.
Curl noise is the analytic curl of a sinusoidal vector potential, with a birth
phase; it is divergence-free and deterministic, with no sampled noise texture.
The particle moves using its incoming velocity, then update modules execute in
authored order. Gravity therefore changes the next tick's displacement.
Plane collision projects penetrations out and reflects inward velocity.
Depth collision needs `DepthQuery` on CPU or an imported scene depth on GPU.
SDF collision is reserved for the later SDF service.

`event_spawn` observes `collision` or `death`, names a target emitter and specifies
`count`. Event positions are copied before slot reuse. Spawn happens at the next
tick, before update. Initializer offsets are relative to the event position.
The GPU event route is entirely GPU-side and cosmetic. Only `CpuSystem` events
are gameplay-authoritative. `CpuSystem` is a Bevy ECS component; fixed ticks have
no wall clock, global RNG or unordered iteration. Never store gameplay state in
the renderer, a C# object or a static.

Register `ara_graph::register` and `commands::register` with the shared schemas
and command registry. Initial imports use `Effect::to_graph`; live edits use
`vfx.text.apply`, `vfx.stack.edit` or `vfx.parameter.set`. These produce kernel
ops with attribution, dry-run, revision checks and undo/redo. Module names are
stable graph identity, so moving a module preserves IDs and layout. Renaming in
text intentionally creates a new identity. Graph edges must agree with stack
order; contradictory or unknown graphs remain stored but cannot compile.

The native stack panel can display each `Stacks::STAGES` slice directly and send
the same commands as agents. Show `Effect::diagnostics` and GPU stats beside it.
Compile `generate` away from the editor in a below-normal-priority worker, cook
Slang to SPIR-V/DXIL, and swap only a complete matching cohort at a frame boundary
under the human's D20 application policy. Failed cooks retain the last good
cohort. See [`ara-vfx-gpu`](../ara-vfx-gpu/README.md) for preview/integration.
