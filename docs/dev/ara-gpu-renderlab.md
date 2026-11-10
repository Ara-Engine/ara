# Renderlab GPU prerequisites

Work on `t3/ara-gpu-gaps`, [PR #5](https://github.com/Ara-Engine/ara/pull/5).
The API keeps one bindless heap, 32-byte roots and explicit graph resource
reachability. This adds prerequisites; it does not migrate Renderlab itself.

## Feature status

Windows hardware verification and fixes are recorded below. The Mac runs shared
API/pure tests, not a Vulkan device; native Vulkan compilation
and execution belong to the PC pass.

| Gap | Implementation and Mac verification |
|---|---|
| Buffer fill and large writes | Aligned u32 fill; ordered updates above 64 KiB use completion-gated staging chunks. Metal in-flight/range probes pass. |
| Formats | All requested formats map on both backends; R32Float already existed. Metal per-mip transfer probes pass. |
| Samplers | Immutable regular/comparison tables, nearest/linear, all three addresses and up to 16x anisotropy. Native MSL and Vulkan filtering/comparison probes pass; fresh Converter 4.0.3 artifacts are available. |
| Images | Volumes, arrays, mip/layer views, whole and rectangular transfers. Metal copy and shader-visible view probes pass. |
| MSAA | Four samples, backed out-of-pass resolves and transient in-pass color resolves. Metal pixel probes pass. |
| Raster | Depth comparisons/bias, cull/winding, vertex layouts and instance streams, per-target blend. Metal draw/depth/blend pixel probes pass. |
| Buffer heap | Up to 4,096 storage buffers, clamped to Vulkan device limits. Metal high-slot shader probe passes. |
| Dynamic ray tracing | Reserved TLAS capacity, rebuild/refit and BLAS compaction. Existing cooked query probes exercise 4,096 instances and changed geometry/transforms on both queues. |
| Ray-query hit vertex positions | `HitGeometry::publish` plus the shared shader helper fetch retained indexed object-space positions. Native Metal query/geometry test passes; the direct intrinsic remains unavailable for DXIL. |
| Compute-only family optional | Vulkan prefers it, then falls back to graphics; pure selection tests pass. This has no equivalent Metal family requirement. |
| Barriers | Batch commands and checked byte/mip/layer ranges. Vulkan emits one dependency call; Metal conservatively synchronizes stages. Native transfer/range probes pass. |
| Memory | Private resources suballocate Metal heaps/Vulkan blocks; completion gates reuse. Metal native heap sharing, preparation ownership and blocked-retirement probes pass. |

## API details that affect the migration

- `TextureDesc` extends `ImageDesc` with `ImageShape`. Default shape preserves
  single-level 2D behavior. `copy_byte_len()` sizes whole portable-format copies:
  finest mip first, packed pixels, four-byte-aligned depth mip starts. Padding is
  untouched. `ImageRegion` copies select one mip and rectangular layer/depth ranges.
- `sampled_view` and `storage_view` return immutable heap slots. The parent image
  remains the graph/lifetime identity. Storage views expose one mip.
- `BeginRenderingResolved` names one optional resolve per color attachment.
  `ImageShape::transient` is attachment-only; resolve before leaving that pass.
  Out-of-pass `ResolveImage` requires backed color storage and transfer usage.
- `RenderState::raster` defaults preserve old cooked states. Explicit vertex layouts
  use `vertex_stride: 0`; `DrawVertexBuffers` supplies streams and instance offsets.
- `sampler()` returns an index in the regular or comparison table according to
  `SamplerDesc::compare`. Vulkan bindings are 2 and 6; DXIL registers are s0 space0
  and s0 space5. Each table holds 64 immutable entries. Regular defaults occupy 0..3.
- New cooks use binding ABI v4. New runtimes also accept v3 cooked artifacts: the
  existing top-level layout is a prefix, with the comparison pointer appended at
  byte 72. Older runtimes reject v4. Buffer-heap growth itself does not change ABI:
  the old `storage128` fingerprint referred to storage images, not storage buffers.
- BLAS update/compaction flags are fixed at creation. TLAS rebuild may change the
  active count within its reservation; refit preserves count. Compaction is blocking
  setup work returning a fresh handle. Repoint TLAS instances before retiring the
  source BLAS. A compacted BLAS can refit but cannot rebuild in place.
- Metal heaps and Vulkan free ranges are retained until both timelines complete.
  Host buffers, external/transient storage and required-dedicated allocations stay
  separate. Vulkan buffer/image blocks are segregated to respect granularity.

## Verification and remaining work

With `CARGO_BUILD_JOBS=2`, the Mac passed:

```sh
cargo fmt -p ara-gpu -p ara-shader-cook -- --check
cargo clippy -p ara-gpu -p ara-shader-cook --all-targets -- -D warnings
MTL_DEBUG_LAYER=0 MTL_SHADER_VALIDATION=0 cargo test -p ara-gpu -p ara-shader-cook -- --test-threads=1
MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --lib metal -- --test-threads=1
MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --test gaps -- --test-threads=1
cargo check -p ara-landscape --example cook
```

The validated Metal library run passed 41 tests (four opt-in/helper tests ignored);
all six portable gap probes passed. Dynamic acceleration tests pass on graphics
and compute. The full suite also presents ten SDL frames and imports an IOSurface
from another process. Shader cooker structural/dependency tests pass; its DXIL
integration tests remain explicitly ignored on macOS.

The opt-in native allocation probes run through `ara-dev gpu`. The buffer-only
fixture requested 552956640 bytes; separate allocations took 592412672, pooled heaps
562036736. A mixed 1080p fixture now covers **1,324 resources**: 512 vertex/index
pairs, 256 mipmapped material maps and 44 render targets, depth pyramids and volume
LUTs. The count/scale matches the earlier PC test; this is a new synthetic inventory,
not a capture of that PC resource set.

| Mixed inventory | Native bytes | Over requested |
|---|---:|---:|
| Requested texels/buffer bytes | 553379568 | — |
| Separate native allocations (before suballocation) | 573769096 | 3.68% |
| Original fixed 8 MiB heaps | 586678272 | 6.02% |
| Tuned heaps (small blocks; matching heaps from 1 MiB) | 569245696 | 2.87% |

A reconstruction of the [original recipe](https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/docs/gpu-layer-bench.md#L257-L266)
also ran: 1,000 log-spaced storage buffers (256 B to 1 MiB), 24 1080p RGBA16Float
storage/sample images and 300 64×64 RGBA8 sampled images. The source does not give
the original buffer rounding; this fixture specifies four-byte rounding, so it is
not a byte-identical replay. Requested 529479412 bytes; separate native allocations
544722772 bytes (+2.88%); pooled heaps 538443776 bytes (+1.69%), 40 heaps. Run
`historical_allocation_footprint` to reproduce `target/ara-gpu/historical-allocation.json`.

The realistic mix exposed wasted tails in medium image heaps. Matching their heaps
to the native requirement saves 17432576 bytes versus the first pool, and 4523400
bytes versus separate allocations. The final set has 59 heaps and 4964352 unused
heap bytes. This measures native allocation reservation, not resident physical pages
or full renderer memory (device tables, host staging, AS and driver caches excluded).
It does not reproduce or supersede the PC's 16.7% Vulkan result. The guard explicitly
allowed foreign load for these allocation-size counters, not timing claims.

Reproduce with the opt-in `mixed_allocation_footprint` library test through
`ara-dev gpu --foreign-cpu-percent 100 --foreign-gpu-percent 100`; it writes canonical
JSON under `target/ara-gpu/mixed-allocation.json`.

The quiet CPU barrier measurement completed on 2026-11-10 after earlier attempts
were refused for foreign CPU load. It alternates 200 AB/BA measured pairs of 256
barriers after 20 warm-up pairs. Pass timestamps are disabled; host record/submit
measurements exclude GPU waits. Both variants execute the same dependencies.

| 256-barrier variant | Record p50 (µs) | Submit p50 (µs) | Total p50 (µs) | Total p95 (µs) |
|---|---:|---:|---:|---:|
| Individual commands | 3.291 | 73.916 | 77.250 | 87.375 |
| One batch | 0.500 | 14.292 | 14.833 | 32.709 |

The total median improves about 5.21× for this microbenchmark, not for a full frame.
`ara-dev gpu` returned **valid** with unchanged default thresholds: foreign CPU peak
5.64%, GPU peak 1.02%. Raw samples are in `target/ara-gpu/barrier-timing.json`;
guard evidence is `target/ara-dev/gpu-load/71855-1794312745221.json`.

Compile the library tests first, locate the test executable with Cargo's JSON
artifact output, then run that executable's `barrier_recording_timing --ignored
--nocapture --test-threads=1` through `ara-dev gpu --timeout 30 --no-wait --`.
Set `MTL_DEBUG_LAYER=0 MTL_SHADER_VALIDATION=0`; running Cargo inside the lease would
include compilation load. The [WGSL replay report](renderlab-wgsl-replay.md) records
separately guarded GPU pass costs.

Windows is the canonical shader cook host and has Metal Shader Converter 4.0.3.
Fresh v4 artifacts are being produced there, then verified on Metal; no Mac install
is needed. Native MSL tests already cover regular and comparison tables. The direct
Slang `CommittedGetIntersectionTriangleVertexPositions()` probe compiles for SPIR-V
but is rejected for DXIL (including `sm_6_10`) by Slang 2026.18.3 with E36107,
“unavailable features in entry point,” before the converter runs. The shared shader
helper below reconstructs the triangle from committed instance/primitive IDs and
retained vertex/index heap entries; the direct intrinsic remains unavailable on DXIL.

On Windows, run the focused gates above without the Metal environment variables,
plus the native allocation test:

```sh
cargo test -p ara-gpu --lib native_private_allocations_share_blocks_without_overlapping -- --test-threads=1
```

The test is ignored only on macOS. Exercise a physical shared-family device too;
selection tests alone cannot prove its queue submission/ownership path. Re-cook
shaders on the canonical host; never merge generated cooked artifacts.

`Command::BeginRenderingSubresources` selects one 2D mip/array slice per color,
depth and resolve output through `RenderAttachment` / `RenderTarget`. Metal native
clear/draw/resolve tests prove sibling contents remain unchanged and invalid ranges
are rejected. Load/store analysis tracks mip/layer discard ranges. The new command
uses cached single-mip/slice views on Vulkan too; its device probe checks draws,
depth writes, resolves, preserved siblings and invalid ranges. See the
[migration design](../design/renderlab-on-ara-gpu.md).

Decision override: D36 supersedes S1's dedicated compute-family assumption.
No other locked decision is overridden.

`hit_geometry::SHADER` works unchanged as Slang source and through its three MSL
adapter macros. `HitGeometry::publish` encodes the same AS-validated position/index
range in 32 bytes, including offsets/stride/counts and heap slots. Records use native
TLAS instance ordinals, not custom IDs. The validated Metal test hits primitive 1 in
instance 1 with padding, nonzero offsets, reordered indices and a mirrored/scaled
transform, then checks all nine original object-space coordinates exactly. Instance
and primitive overflow return invalid. The helper also compiles to SPIR-V locally;
The PC also cooked the helper through DXIL/Converter 4.0.3 and verified these exact
coordinates on Vulkan. Its flat probe output uses scalar stores: this PC's Slang
lowers `Store3` through a std430 uint3 array with 16-byte strides, rather than packed
12-byte rows. The shared retained-geometry helper itself is unchanged.

The [naga post replay](renderlab-wgsl-replay.md) now proves bloom downsample,
upsample and post tone on Metal from the exact WGSL shader bodies, with CPU
references and no Slang/MSC. Generated bindless SPIR-V validates; native Vulkan
execution is pending. `ara-gpu-replay` is a test-only dependency, not a change to
the production shader loader.

## Windows verification, 2026-11-10

Device: NVIDIA GeForce RTX 4070 SUPER, driver 617.14; Vulkan SDK 1.4.363.0.
Debug tests require `VK_LAYER_KHRONOS_validation` and synchronization validation;
`ARA_NO_GPU_VALIDATION` was unset. All runs were headless in isolated worktrees,
with two Cargo jobs, a separate D: target and GPU leases of at most 14 minutes.

| Feature | Native Vulkan evidence |
|---|---|
| Fill / large staged writes | `gaps::large_updates_keep_both_in_flight_payloads_and_fill_patterns` checks exact bytes and in-flight ordering. |
| Formats | Ten transfer formats plus odd D16 / half-float mip alignment in `gaps`. |
| Samplers | `comparison_sampler` exercises binding 6; `device_features` checks nearest/linear, repeat/mirror/clamp and a 16x sampler through shader reads. |
| Volumes / arrays / mips / regions | Whole and interior 3D transfers in `gaps`; `device_features` reads sampled array subviews and writes storage subviews, preserving siblings. |
| 4x MSAA / resolve | Backed and transient resolves in `gaps`; raster draws and selected-layer resolves in `raster` / `render_targets`. |
| Raster / vertex state | Per-vertex and per-instance streams with offsets, first instance, cull/winding, depth comparison/bias and independent MRT blending. |
| Larger storage-buffer heap | `sampled_heap` reads a slot above 1,024 on the device; selected capacity is logged. |
| Dynamic acceleration | `dynamic_acceleration` checks 4,096 TLAS instances, rebuild/refit/compaction on graphics and compute; `hit_geometry` checks retained positions from a real hit. |
| Shared-family fallback | The library test forces the actual graphics family for both classes and checks synchronous work plus deferred compute/graphics dependencies on aliased native queues. This RTX still physically offers a dedicated compute family. |
| Batched / ranged barriers | Buffer and image range probes check bytes, selected mips/slices and rejected overflow. |
| Suballocation | `native_private_allocations_share_blocks_without_overlapping`, preparation, disposal and deferred-lifetime tests pass on hardware. |

The run found a storage-descriptor retirement bug: an image's default sampled view
was used as its storage-heap identity, although storage publication uses a separate
view. Retirement and full-heap reclamation now use the published storage view.
Custom storage views also trigger collection when their heap fills; both paths have
native exhaustion/reuse tests. Rendering-subresource support was implemented in
Vulkan without changing the portable API.

The original MSAA clear assertion assumed round-to-nearest UNORM conversion.
Vulkan permits either adjacent integer ([conversion rules](https://docs.vulkan.org/spec/latest/chapters/fundamentals.html#fundamentals-fp-conversion));
the test now requires a uniform full image, legal adjacent values and exact endpoints.
The RTX returns `[64,127,191,255]` for `[0.25,0.5,0.75,1]`.

All four requested dependent unit suites passed: `ara-render`, `ara-runtime-render`,
`ara-editor-gpu`, and the `ara-player` binary unit suite. The updated main includes a
pinned Renderlab Git dependency; Cargo built that cached revision, never the live
Renderlab checkout.

The WGSL SPIR-V adapter now normalizes its selected IR entry to `main`, keeping
Ara's loader ABI and the pinned shader bodies unchanged. Replay filenames include
the shader stage so the tone vertex and fragment modules cannot overwrite each
other. Final native replay verification is pending the shared build slot.

Fresh v4 artifacts were cooked with Metal Shader Converter 4.0.3 for 24 cohorts:

- renderer root `shaders` (`ara-render/cooked`);
- `ara-editor-gpu`, `ara-gpu`, `ara-landscape`, `ara-render-fidelity`,
  `ara-render-geo`, `ara-render2d`, `ara-runtime-render`, `ara-texture-gpu`,
  and `ara-texture-stream` shader cohorts;
- `ara-gpu/tests/shaders` and `ara-gpu/tests/feature_shaders` (including comparison
  sampling, raster/attachment selection and the retained ray-hit helper);
- VFX `campfire`, `depth_sparks`, `dust`, `magic_swirl`, `mesh`, `million`,
  `million_fill`, `muzzle_flash`, `rain`, `ribbon`, `smoke`, and `sparks`.

The artifact audit checked 419 pipelines / 507 stages: every metallib exists,
matches its SHA-256 and records Converter 4.0.3. No converter cook failed. Generated
artifacts were re-cooked after source updates, never resolved by hand. The Mac can
pull them and run the portable `comparison_sampler`, `raster`, `device_features`,
`render_targets`, `hit_geometry`, `gaps` and dynamic-acceleration probes on Metal.
The direct DXIL position-return intrinsic remains the compiler limitation described
above; the verified retained-geometry helper supplies the portable result.

## Metal barrier hardening from the Renderlab port

The two-phase cluster-cull replay exposed a late dependency after a timestamp split:
an unrelated readback opens a compute encoder, then a subsequent copy reads a shader
result from an earlier encoder. An encoder-stage barrier orders only work inside
its own encoder. The lowering now also applies pending queue-stage waits immediately
when a dependency is discovered in an already open encoder. Remaining render-stage
waits stay pending for the matching encoder.

A focused native MSL regression reproduces the original stale-zero read, then checks
eight distinct producer/readback snapshots after the fix. The Metal suite passes
45 tests with API/shader validation (7 timing/manual tests ignored); the full library
passes 79 tests with API validation and shader validation disabled (18 ignored).
`cargo clippy -p ara-gpu --all-targets -- -D warnings` and the 117-package formatting
checkpoint pass. This fix changes Metal lowering; the Vulkan implementation remains
with the PC verifier. It changes no locked decision or shader ABI.
