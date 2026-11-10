# Renderlab WGSL on ara-gpu: post replay

2026-11-10, M5 Pro / macOS 27, naga 30.0.1. Test-support crate:
[`ara-gpu-replay`](../../crates/ara-gpu-replay/README.md). GPU runners live in
[`replay_tests.rs`](../../crates/ara-gpu/src/replay_tests.rs) and
[`metal/naga_replay_tests.rs`](../../crates/ara-gpu/src/metal/naga_replay_tests.rs).

**Result: yes for these three pass families.** Bloom downsample, bloom upsample
and post tone execute on Ara's Metal 4 device and existing bindless heap from naga's
MSL, with no Slang rewrite, DXIL or Metal Shader Converter. All four generated
SPIR-V stages pass `spirv-val --target-env vulkan1.3`. Native Vulkan execution and
its sampled-descriptor aliases remain unverified until the PC run.

This is a test-only frontend experiment. The production loader/cooker and A8's
Slang path remain unchanged. It does not certify every Renderlab shader/program,
ray-query operation or full-frame backend parity.

## Source and workload

The two WGSL files are byte-for-byte copies of Renderlab's
[`post.wgsl`](https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post.wgsl)
and [`post_tone.wgsl`](https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post_tone.wgsl).
Their arithmetic, workgroup sizes, clamping, tent weights, 0.65 upsample blend and
tone/grade/LUT/vignette logic are generated from WGSL rather than rewritten in MSL.
The host fixture disables grain and chromatic aberration; those shader branches
remain in the copies but are not verified by the scalar oracle.

The deterministic HDR input is 17×11 RGBA16Float, with a gradient and bright impulse.
Downsample produces 9×6 then 5×3; upsample combines those into 9×6; fullscreen tone
reads the original HDR, upsample output, and a nonidentity 4³ half-float LUT, and
writes 17×11 RGBA32Float. Settings enable filmic tone, white balance, contrast,
saturation, LUT grading and vignette. Every intermediate image and the final linear
float image is read back and checked against an independent scalar implementation.
The scalar path rounds each bloom store through f16.

| Output | Maximum absolute component error on Metal |
|---|---:|
| Downsample 9×6 | 0.001953125 |
| Downsample 5×3 | 0.0009765625 |
| Upsample 9×6 | 0.001953125 |
| Tone 17×11 | 0.00050473213 |

The threshold is 0.003, with finite-value checks, against half-float intermediates
and hardware filtering. This is numerical agreement within that tolerance, not
byte identity. The reference includes clamp edges, odd sizes, the impulse and 3D
LUT coordinates. A separate scalar test checks constant-energy preservation.

## What had to change

No per-pass descriptor set or native texture/sampler binding was added. Eight root
words carry input/colour, second/bloom, auxiliary, velocity tiles, writable output,
settings buffer, sampler and LUT heap indices. Settings remain the same five vec4s
(80 bytes), transported through a read-only storage-buffer slot instead of a UBO.
Commands declare transitions between uploads, each compute output, fragment reads,
color output and readback. Both backend runners use the same resources and commands.

**MSL:** naga initially emits fixed-resource entry parameters. Before writing MSL,
the adapter changes the settings global's IR address space from Uniform to read-only
Storage and revalidates it; struct offsets remain unchanged. A checked entry shim
passes heap-selected textures, sampler and `device Settings&` to the generated
function, taking only Ara's top-level buffer at Metal buffer index 2 plus the native
invocation/position builtin. Typed 24-byte aliases read writable-2D and sampled-3D
resource IDs from the existing descriptor tables. The fullscreen vertex entry needs
no heap access. The algorithm bodies are untouched.

**SPIR-V:** a checked, pinned-source WGSL resource adapter replaces bindings with
runtime binding arrays: storage buffers at 1, samplers at 2, storage images at 3,
sampled images at 5. Naga's `enable wgpu_binding_array` and `var<immediate>` dialect
represent descriptor arrays and the 32-byte native push constants. References to
resource globals become array selections by root word. Replacement is limited to
the known resource globals; compute's local `colour` must not be replaced.

Naga's source validator disallows differently typed globals at one binding. The
3D LUT therefore uses temporary binding 7 during generation; its final SPIR-V
Binding decoration is changed to sampled binding 5, aliasing the same descriptor
array as the 2D textures. This does not use the legacy binding-0 / first-128 mirror.
The selected view must match the type at each accessed index. Standalone validation
passes, but only native Vulkan validation/execution can finish proving that alias.

Two naga 30 output repairs were also necessary:

- Runtime arrays were emitted without `RuntimeDescriptorArray`; the adapter inserts
  that capability only when missing. Without it, Vulkan validation reports VUID-04680.
- Functions taking a texture parameter expect a UniformConstant image pointer, but
  binding-array calls supplied the loaded image value. The adapter replaces that
  argument with its originating pointer only when the callee's exact pointer type
  matches. It does not cast unrelated values or rewrite arithmetic. The repaired
  stages validate without adding VariablePointers capabilities.

These are bounded adapters for the pinned shaders/naga version, not a general WGSL
heap compiler. A production implementation should use structured IR/backend lowering
and address the naga issues upstream; preserve checked failure on an unexpected
signature rather than silently applying text edits to arbitrary user programs.

## Reproduce

With `CARGO_BUILD_JOBS=2`:

```sh
cargo test -p ara-gpu-replay
MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --lib naga_msl_replays -- --nocapture
cargo test -p ara-gpu --lib emit_naga_spirv -- --nocapture
```

The emitter prints its unique output directory under `target/ara-gpu/wgsl-replay/`.
Run SDK `spirv-val --target-env vulkan1.3` on its four `.spv` files. Source MSL,
comparison JSON and metadata also stay under that target tree; PID/sequence output
roots prevent default-parallel tests from racing shader writes and hash checks.

On Windows, execute the generated SPIR-V through the existing pipeline loader:

```sh
cargo test -p ara-gpu --lib naga_spirv_replays_renderlab_post_against_cpu -- --nocapture
```

It generates fresh artifacts locally, uses a separate artifact root, executes the
same chain on Vulkan, checks the same CPU references, and calls `assert_clean`.
Slang/Metal Shader Converter cooking is not involved in this path.

For timing, first compile tests, then run only `naga_post_pass_timing` through
`ara-dev gpu` with `MTL_DEBUG_LAYER=0 MTL_SHADER_VALIDATION=0`. It warms eight frames
and records 20 samples per pass, using GPU timestamps outside setup/readback work.
The image sizes are tiny: any reported cost is this replay's launch/filter cost,
not a full-resolution renderer claim. Accept timing evidence only if the default
foreign-load guard returns valid.

On 2026-11-10, the guarded run returned **valid** at the unchanged default thresholds
(foreign CPU peak 7.70%, GPU peak 1.37%). GPU timestamp results:

| Stage | Output extent | Median (µs) | p95 (µs) |
|---|---|---:|---:|
| Bloom downsample 0 | 9×6 | 19.542 | 20.000 |
| Bloom downsample 1 | 5×3 | 20.917 | 21.667 |
| Bloom upsample | 9×6 | 20.167 | 21.958 |
| Post tone | 17×11 | 27.708 | 29.625 |

These tiny dispatch/draw costs do not predict 1080p throughput or compare against
wgpu. Setup, native MSL compilation, upload and readback are outside the GPU pass
timestamps. The eight warm-up frames precede 20 measured frames; every frame also
passes the CPU-reference assertions. An earlier attempt was refused for load and
is excluded. Raw timing output is under
`target/ara-gpu/wgsl-replay/timing-74604-31/`; accepted guard evidence is
`target/ara-dev/gpu-load/74554-1794313060519.json`.

## Limits and next decision

The proof supports keeping Renderlab WGSL for this first migration slice. It removes
MSC and Slang as prerequisites for these kernels. It does not yet prove instanced
vertex fetch/base-instance conventions, divergent material descriptor indices,
MSAA/depth/atomic/ray-query shaders, grain/aberration, sRGB output conversion, shader
reload or production artifact packaging. The test compiles generated MSL with the
OS API; production still needs a deliberate offline/cache/loader path for native
naga output. Continue with the next isolated kernels and hard-feature probes before
making a renderer-wide authored-language or default-backend decision.

Before item 3, this checkout's approximately 1.4 GiB incremental compiler cache was
deleted after confirming it was inside this worktree. No other worktree/build output
was touched. The replay added roughly half a GiB of focused build output; disk space
remained above 30 GiB during its gates, so disk did not block the experiment.
