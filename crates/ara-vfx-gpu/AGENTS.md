# ara-vfx-gpu

GPU-owned particle simulation and procedural sprite/ribbon/mesh output, expressed
as ara-render graph passes. ara-vfx owns authoring and gameplay-authoritative CPU
events. Shader compilers are never runtime dependencies.

## API

- `GpuSystem`: allocate from a matching cooked effect, step fixed ticks, render,
  capture through ara-render's channel/provenance API, return agent stats.
  `advance` simulates a tick without the backdrop copy, draw or tonemap: live
  composition advances by elapsed ticks, then renders once per presented frame.
- `composite`: blend into supplied scene HDR, optionally advancing a tick; no
  preview backdrop or display tonemap. The scene owns the final look post.
- `PipelineWarmup`: poll bounded worker compilation; live frames omit the effect until ready.
  Consume the ready batch with `GpuSystem::new_prepared`, or retire/cancel it explicitly.
  Native pipeline compilation and its private cache I/O never run on the frame thread.
- `View`: camera/frame inputs; optional scene HDR and depth for world composition.
  `placement` (world origin + uniform scale) maps the world camera, scene depth and
  probe lattice into effect space; `intensity` scales particle radiance.
- Lit soft/smoke sprites shade as thin media (HG phase g=0.35 + probe mean
  radiance), not Lambert surfaces; spark sheets, meshes and ribbons keep surface light.
- `GpuSystem::capture_over(gpu, request, scene)`: `capture` composited over a scene
  view (background, depth, lighting) for every replayed frame (play captures).
- `Lighting`, `ProbeLighting`: sun/ambient and completed fidelity DDGI inputs.
- `GpuSystem::render`, `hdr_output`: frozen-tick HDR composition before fidelity post.
- `read_coverage`: explicit diagnostic readback of actual GPU feature IDs.
- `cook_atlas`: original offline flipbooks; committed RGBA8 density/gradient assets.
- `cook_source`: offline specialized module source plus material-system shaders.
- `examples/vfx_cook`: external Slang/SPIR-V/DXIL cooker for the sample cohort.
- `examples/vfx_capture`: deterministic previews and reproducible channel hashes.
- `examples/vfx_timings`: warmed million-sprite GPU timings at 2560x1440.

## Layout and synchronization

Particles are four SoA float4/u32x4 channels. Each emitter has a padded power-of-two
partition; the requested capacity and effect quality cap bound active slots.
Alive/dead compaction uses group scans and deterministic group-prefix sums.
Projected-coverage prefixes bound the selected draw list before depth/age sorting.
Events copy positions before slot reuse, route on GPU, spawn on the next tick.
Draw/dispatch arguments are GPU-generated; GPU events are cosmetic only.
Depth/age sorts use deterministic bitonic passes with slot-ID ties over the selected
live prefix's next power of two. Compaction writes indirect work sizes into the
existing control buffer tail; capacity-only stages dispatch zero work. Additive
sprites/meshes skip sorting; ribbons retain age order. No per-frame CPU readback.
All heap-reachable resources must appear in graph accesses. Frame resources are
owned; no mutexes, interior mutability or per-frame resource clones. Frame constants
use `upload_queued` so an enclosing deferred Scene scope orders writes on the GPU.
Constructors initialize host rows before submitting transfers and retire partial
resources/pipelines on failure. `tests/deferred.rs` checks this with a blocked
atlas transfer and repeated descriptor-exhaustion retries.
Async compute is opt-in and must be measured on the target workload.
Shader/pipeline replacement happens at frame boundaries after worker completion.

## Checks

Set CARGO_BUILD_JOBS=2. `run.ps1 -Mode Check` scopes the renderer's implicit-layer
workaround and runs crate gates. Capture/timing modes run headlessly.
Ordinary cargo test includes CPU contract tests; GPU goldens are explicitly
enabled with ARA_VFX_GPU_TESTS=1. They require this device/driver and SDK layers.
GPU test captures and measurements go under target/ara-vfx-gpu; tracked evidence
and goldens stay unchanged unless the explicit golden update flag is set.
ARA_UPDATE_GOLDENS=1 deliberately updates six still/sequence PNG baseline pairs.
Inspect captures before accepting them. Gallery frames use lit geometry and the
fidelity post stack; benchmark sprites intentionally isolate simulation/fill cost.
Cooked artifacts are committed. A fresh check/test never runs a shader compiler.
