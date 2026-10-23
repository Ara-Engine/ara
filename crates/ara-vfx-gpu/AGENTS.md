# ara-vfx-gpu

GPU-owned particle simulation and procedural sprite/ribbon/mesh output, expressed
as ara-render graph passes. ara-vfx owns authoring and gameplay-authoritative CPU
events. Shader compilers are never runtime dependencies.

## API

- `GpuSystem`: allocate from a matching cooked effect, step fixed ticks, render,
  capture through ara-render's channel/provenance API, return agent stats.
- `View`: resolved camera/frame inputs; optional imported scene depth.
- `cook_source`: offline specialized module source plus material-system shaders.
- `examples/cook`: external Slang/SPIR-V/DXIL cooker for the sample cohort.
- `examples/capture`: six deterministic previews and reproducible channel hashes.
- `examples/timings`: warmed million-sprite GPU timings at 2560x1440.

## Layout and synchronization

Particles are four SoA float4/u32x4 channels. Each emitter has a padded power-of-two
partition; the requested capacity and effect quality cap bound active slots.
Alive/dead compaction uses group scans and deterministic group-prefix sums.
Events copy positions before slot reuse, route on GPU, spawn on the next tick.
Draw/dispatch arguments are GPU-generated; GPU events are cosmetic only.
Depth/age sorts use deterministic bitonic passes with slot-ID ties.
All heap-reachable resources must appear in graph accesses. Frame resources are
owned; no mutexes, interior mutability or per-frame resource clones.
Async compute is opt-in and must be measured on the target workload.
Shader/pipeline replacement happens at frame boundaries after worker completion.

## Checks

Set CARGO_BUILD_JOBS=2. `run.ps1 -Mode Check` scopes the renderer's implicit-layer
workaround and runs crate gates. Capture/timing modes run headlessly.
Ordinary cargo test includes CPU contract tests; GPU goldens are explicitly
enabled with ARA_VFX_GPU_TESTS=1. They require this device/driver and SDK layers.
ARA_UPDATE_GOLDENS=1 deliberately updates the six PNG baselines.
Cooked artifacts are committed. A fresh check/test never runs a shader compiler.
