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
| Ray-query hit vertex positions | **Skipped:** direct position fetch is not enabled or exposed. Metal converter tooling/authentication blocks proving the portable shader contract. |
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

The opt-in native allocation-footprint probe ran through `ara-dev gpu`. Raw bytes:
requested 552956640; separate native allocations 592412672; pooled heaps 562036736
across 67 heaps. This is a Mac allocation-count measurement, not the original PC
resource set or a timing result. The guard explicitly allowed foreign load because
this records native allocation sizes. Quiet CPU barrier timing was deferred because
other editor/player and GPU work was active; no speedup claim is made.

The Metal Shader Converter download redirects to Apple Developer sign-in, where
interactive password/passkey authentication is required. No converter was installed.
A fresh v4 converter cook (including comparison sampling) and direct ray-query hit
position fetch remain blocked. Existing cooked v3 Metal shaders still run and prove
prefix compatibility; native MSL tests cover the new table behavior.

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
