# Software secondary transport experiment (B4 / G6)

Disabled unless a host calls `Renderer::set_software_reflections(Some(settings), &scene)`.
`None` retires its resources and restores the default path. There is no preset/default
cut: `SoftwareReflections` requires explicit mirror and rough cuts, ray count, distance
and interface budget. Hardware secondary queries and patch readout must be disabled.
The experimental primary frame requires native resolution and bypasses jitter, TSR and temporal
accumulation in the final resolve. Secondary views, capture/planar bakes and default
frames retain their existing behavior.

`scene.rs` adapts native LOD 0 geometry and instances to A1's shared `TraceScene` and
`TraceGpu`. Geometry changes use the content-addressed BLAS cache; transforms refit.
Replacing the CPU scene republishes the GPU scene even if its version numbers repeat.
`ARA_REFLECT_CACHE` overrides the cache under the workspace's `target/` directory.

The new `reflect-*` and `glass-*` shaders use `traceClosestWith` and `traceAnyWith` from
`trace.slangh`. Their alpha callback reads current native texture residency, UV transforms,
material alpha, vertex colours and per-instance paint. A1's generic sampled-image alpha
table is only a cutout marker here; native texture IDs are not sampled-heap indices.

The mirror band traces one direction per covered pixel. The rough band uses fixed
Hammersley coordinates over the visible GGX distribution, correlated Smith/Fresnel
weights and a fixed sample-count denominator. There is no pixel phase, RNG, history,
spatial reconstruction or screen-colour input. Hit lighting uses native materials,
existing direct-light BRDFs/shadow maps and today's probe GI. Unmapped local lights
use software any-hit visibility. The upper band uses today's prefiltered sky, probe
volume and reflection captures; it is not B3's future patch lobe.

Ordered glass follows reflected and transmitted branches through distance-ordered
interfaces, with Fresnel, Snell refraction and Beer-Lambert attenuation. Four nested
closed media and at most sixteen interfaces per branch are representable. Thin walls
leave parallel. No small-throughput cutoff removes branches. Exhaustion and invalid
medium nesting are reported; truncation is not a convergence claim. Glass interfaces
currently use smooth normals rather than a GGX interface quadrature. Terrain projection,
decals at secondary hits, camera-inside-medium initialization and a certified remaining
path-energy bound are not implemented. These limitations prevent general promotion.

`Renderer::software_reflection_counters()` reads completed work on explicit request:
mirror/rough/prefiltered pixels, all secondary rays, any-hit rays, visited interfaces,
exhausted branches and invalid media. It submits a readback barrier, so callers measure
the frame span before requesting the counters. GPU stages are `Reflect.Clear`,
`Reflect.Mirror`, `Reflect.Rough`, `Reflect.Prefiltered`; ordered-glass raster work is in
`ForwardTransparency`. Existing control passes still execute where the normal graph
declares them, and their cost remains part of the complete candidate frame.

## Checks and measurements

Check the shared quiet-window lock before every Cargo/GPU command. Cook under the
shared cook lock. Set `CARGO_BUILD_JOBS=4` for this shared render-v3 checkout.

```text
cargo clippy -p ara-render -p ara-render-fidelity --all-targets -- -D warnings
cargo test -p ara-render look::reflect -- --test-threads=1
cargo build --release -p ara-render-fidelity --example reflect_g6
reflect_g6 CASE.json capped OUTPUT 2560 1440 256
```

The example compiles T1's fixture, replay and metrics modules unchanged. `uncapped`
records timing without references; it retains the control renderer's documented hidden
limits. Use the quiet-window lock and `ara-dev gpu` for timing. `capped` produces native
metrics and at-most-1080p report PNGs, with finite undenoised eight-bounce references.
Those references are not certified Cycles solutions. `ARA_B4_REFERENCE_CACHE` reuses
identical reference inputs, independently of reflection settings.

Experiment inputs are `ARA_B4_MIRROR_CUT`, `ARA_B4_ROUGH_CUT`, `ARA_B4_RAYS`, and
`ARA_B4_INTERFACES`. `ARA_B4_CONTROL=1` selects the unchanged same-binary control.
`ARA_B4_FRAME_LIMIT` is for cut sweeps only; complete receipts use the full case script.
The optional `all-references` final argument evaluates every frame. Supplemental
Showcase captures set `ARA_B4_SHOWCASE` to the pinned project and optionally
`ARA_B4_SHOWCASE_SHOT` to an authored camera. They reuse the lab's fixed post settings,
not the editor's bloom/filmic stack, and are kept separate from the three G6 cases.
