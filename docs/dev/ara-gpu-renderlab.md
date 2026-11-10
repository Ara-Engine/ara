# Renderlab GPU prerequisites

Work on `t3/ara-gpu-gaps`, [PR #5](https://github.com/Ara-Engine/ara/pull/5).
The API keeps one bindless heap, 32-byte roots and explicit graph resource
reachability. This adds prerequisites; it does not migrate Renderlab itself.

## Feature status

Every Vulkan implementation below is **native-unverified** until run on Windows.
The Mac compiles those paths and runs their pure tests, not a Vulkan device.

| Gap | Implementation and Mac verification |
|---|---|
| Buffer fill and large writes | Aligned u32 fill; ordered updates above 64 KiB use completion-gated staging chunks. Metal in-flight/range probes pass. |
| Formats | All requested formats map on both backends; R32Float already existed. Metal per-mip transfer probes pass. |
| Samplers | Immutable regular/comparison tables, nearest/linear, all three addresses and up to 16x anisotropy. Native MSL filtering/comparison probes pass; new converter cook remains blocked below. |
| Images | Volumes, arrays, mip/layer views, whole and rectangular transfers. Metal copy and shader-visible view probes pass. |
| MSAA | Four samples, backed out-of-pass resolves and transient in-pass color resolves. Metal pixel probes pass. |
| Raster | Depth comparisons/bias, cull/winding, vertex layouts and instance streams, per-target blend. Metal draw/depth/blend pixel probes pass. |
| Buffer heap | Up to 4,096 storage buffers, clamped to Vulkan device limits. Metal high-slot shader probe passes. |
| Dynamic ray tracing | Reserved TLAS capacity, rebuild/refit and BLAS compaction. Existing cooked query probes exercise 4,096 instances and changed geometry/transforms on both queues. |
| Ray-query hit vertex positions | **Skipped:** direct position fetch is not enabled or exposed. Slang’s position-fetch intrinsic lacks a DXIL target; a portable shader helper needs verification. |
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
JSON under `target/ara-gpu/mixed-allocation.json`. CPU barrier timing now alternates
200 AB/BA measured pairs after warm-up and reports recording/submit/total p50 and
p95, excluding GPU waits. Default-threshold attempts refused timing due foreign
CPU load; the latest peak was 24.65% against the 10% limit. No quiet CPU result is
claimed yet; the failed lease evidence is under `target/ara-dev/gpu-load/`.

Windows is the canonical shader cook host and has Metal Shader Converter 4.0.3.
Fresh v4 artifacts are being produced there, then verified on Metal; no Mac install
is needed. Native MSL tests already cover regular and comparison tables. The direct
Slang `CommittedGetIntersectionTriangleVertexPositions()` probe compiles for SPIR-V
but is rejected for DXIL (including `sm_6_10`) by Slang 2026.18.3 with E36107,
“unavailable features in entry point,” before the converter runs. A shared shader
helper can reconstruct the triangle from committed instance/primitive IDs and
retained vertex/index heap entries; native position-return remains unverified.

On Windows, run the focused gates above without the Metal environment variables,
plus the native allocation test:

```sh
cargo test -p ara-gpu --lib native_private_allocations_share_blocks_without_overlapping -- --test-threads=1
```

The test is ignored only on macOS. Exercise a physical shared-family device too;
selection tests alone cannot prove its queue submission/ownership path. Re-cook
shaders on the canonical host; never merge generated cooked artifacts.

Decision override: D36 supersedes S1's dedicated compute-family assumption.
No other locked decision is overridden.
