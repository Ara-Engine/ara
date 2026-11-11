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

At the first checkpoint, with `CARGO_BUILD_JOBS=2`, the Mac passed:

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
from another process. Shader cooker structural/dependency tests passed; DXIL integration was then
ignored on macOS. The joint gate below enables those tests through the SDK CLI.

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

Windows produced the fresh v4 artifacts with Metal Shader Converter4.0.3.
The joint gate below also verifies a complete Mac test-cohort refresh through
Slang HLSL and the SDK DXC CLI. Native MSL tests already cover regular and comparison tables. The direct
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


## Joint Vulkan/Metal gate and complete test-cohort refresh

The coordinator's Linux gate on Radeon760M reported that `e8a836869` passes build,
clippy, replay, storage-heap, shader-cook and RT tests with Khronos/synchronization
validation. Its remaining slope-bias failure came from a stale fullscreen cook.
Those are external Vulkan results for that head, not a Mac Vulkan run or proof of
the subsequent head.

The Mac recooked all five pipelines/seven stages in
`crates/ara-gpu/tests/shaders/pipelines.json`: SPIR-V, DXIL, both Slang reflections,
Converter4.0.3 metallibs and converter reflections. Nothing is missing. The fresh
fullscreen SPIR-V reads root words0 and1 and multiplies the gradient by clip x.
The downloaded Converter4.0.3 pkg runs from an expanded user-writable directory;
no admin install or further Apple sign-in was required. Slang2026.18.3 cannot load
DXC downstream on this Mac; its generated HLSL compiles through the SDK's DXC1.9
CLI before normal conversion. Mac recipe/tool/library fingerprints produce a
distinct cook key. D37 records the expressly requested exception to Windows-only
cook publication. Windows remains the default host.

Running without shader validation exposed another native dependency bug: a barrier
encountered in an active transfer encoder was consumed there, but pass timestamping
could move the shader consumer to a later encoder. The dependency now remains
pending across that split while also ordering the current encoder. A focused
16MiB-copy regression failed before the fix: only one of three sampled words had
arrived. It passes after the fix; the unchanged WGSL post replay also recovers.
Shader validation's serialization had hidden the race.

The focused gate runs `ara-gpu`, `ara-gpu-replay` and `ara-shader-cook`, with
`CARGO_BUILD_JOBS=2`, Metal API validation, and both shader-validation modes.
The complete converter comparison-sampler test, cooked triangle-range/hit-position
helper, slope-bias raster probe, attachment ranges, dynamic AS and post replay are
included. Cooker incremental recovery and permutation tests now run on Mac rather
than skipping DXIL. The all-targets `clippy -- -D warnings` and workspace formatter
are required at this checkpoint. Native Vulkan must retest the resulting PR head.

## Vulkan queue-family review fixes

Main `d8015f178` is merged. A shared-family image handoff now changes the layout
on release only; its acquire uses the new layout on both sides while retaining
destination synchronization scopes. Distinct-family ownership transfers still
encode matching old/new layout pairs and family indices. The pure regression
failed before the fix (`GENERAL` on acquire instead of `SHADER_READ_ONLY_OPTIMAL`).
The forced-shared-family hardware test now dispatches a storage-image producer and
a sampled-image consumer across the two logical queues and checks every output
word, both in an aliased deferred queue and in ordinary submissions.

Vulkan now caches the selected physical families' capability flags and rejects
depth/stencil buffer-image copies on a family without graphics capability. This
covers `UploadImage`, `ReadImage` and both regional forms, before native recording
or submission-state changes. Use `Queue::Graphics` for those copies on a dedicated
compute device; logical compute remains legal when its family supports graphics.
Color copies remain legal on compute-only families. Ara does not enable
maintenance10; the restrictions are documented in
[upload VUID 07739](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdCopyBufferToImage.html)
and [readback VUID 10216](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdCopyImageToBuffer.html).
The pure regression admitted all eight D16/D32 × whole/regional × upload/readback
cases before the fix and rejects them after it. A native-device test forces the
compute-only preflight capability even on a universal-family Radeon, checks all
eight rejections and verifies the submission counter stays unchanged.

Mac checks: the Vulkan-only unit selection passes **25**, with **13** native
hardware tests ignored; all-targets clippy with `-D warnings` and the 117-package
formatter pass. `ara-gpu` plus `ara-gpu-replay` pass **108**, with **31** ignored,
in each Metal API+shader-validation and API-only run. Both original faults have
recorded red/green runs under `target/ara-gpu/vulkan-review-*.log`. No shaders or
artifact cohorts changed, and these fixes override no decision.

Native Vulkan verification of this patch remains pending on Linux Radeon and
Windows NVIDIA. In addition to the full suites, run these library filters with
Khronos and synchronization validation enabled:

```sh
cargo test -p ara-gpu --lib shared_family_submits_both_queue_classes_on_hardware -- --nocapture
cargo test -p ara-gpu --lib forced_compute_only_depth_copies_fail_before_native_submission -- --nocapture
```

## Vertex-only depth pipelines found during migration

Renderlab's transparency sun-shadow pass has only `tp_shadow_vs`, with no fragment
stage. The portable cooked stage list already represents that shape, but Metal's
loader previously admitted only compute or vertex/fragment pairs. It now admits
one vertex stage when the render state has depth and no color formats. The shared
render builder receives an optional fragment descriptor; no dummy fragment shader,
heap ABI change, or cooked-artifact merge is involved. Vulkan code is unchanged.

A native regression failed before the fix at pipeline admission. After the fix it
renders depths0.75,0.25,0.5 with Less over clear1, then reads every8×8 depth texel
as exactly0.25. `ara-gpu` plus replay pass **109**, with **31** ignored, in each
Metal API+shader-validation and API-only run. All-targets clippy `-D warnings`
and the117-package formatter pass. This addition overrides no Ara decision;
native Vulkan execution of the eventual vertex-only WGSL replay remains pending.

The latest CPU timing on the Vulkan-review-fix head0b4308e18 completed200 alternating
pairs after20 warmups. Individual/batched256-barrier total p50 is
**131.416/15.208µs**, p95 **165.667/35.125µs**; record p50 **3.208/0.500µs** and
submit p50 **128.333/14.708µs**. The unchanged guard accepted it: foreign CPU
mean5.82%, peak11.33% without a sustained breach; GPU peak0.64%. This is an isolated
CPU microbenchmark, not a frame-speed claim. Evidence:
`target/ara-gpu/barrier-timing-review.{json,log}` and
`target/ara-dev/gpu-load/90104-1794339026238.json`.

## Cross-family image handoff (follow-up to PR #5)

`distinct_families_hand_a_storage_image_between_queues_on_hardware` (library test in
`vulkan/context/queues.rs`) runs the storage-image handoff probe on the device's real
queue selection: compute to graphics and graphics to compute, each immediate and
under deferred completion. The release and acquire barriers carry two different
family indices there; the forced shared-family test omits them. A Vulkan device whose
graphics and compute classes share one family prints `skipped:` and passes; with no
Vulkan device it fails at device creation, as the sibling probes do. A skip looks like
a pass in the report, so gates known to have two families (NVIDIA, RADV) set
`ARA_REQUIRE_DISTINCT_FAMILIES=1`, which turns the skip into a failure. Run it in a
debug build: synchronization validation is what detects a missing or mismatched
transfer, and the byte read-back cannot, because both families share memory. It is
ignored on macOS like the other Vulkan hardware probes, so it compiles on the Mac but
has not run: the first native result comes from the Linux and Windows gates.

## Fragment barycentrics (found by Renderlab's first Vulkan run)

Renderlab's lit, receiver and glass fragment stages read per-vertex attributes, so
their SPIR-V declares `FragmentBarycentricKHR`. The Vulkan device never enabled
`VK_KHR_fragment_shader_barycentric`, and under validation every such module failed
at `vkCreateShaderModule` (nine Renderlab backend tests on RADV). The device now
enables the extension and its feature whenever the driver reports both, and
`DeviceInfo::fragment_barycentrics` says whether it did; every Metal 4 GPU reports
it through `supportsShaderBarycentricCoordinates`. Where the flag is false, Vulkan
pipeline creation refuses a module that declares the capability with a contract
error naming the stage, instead of handing it to the driver. A driver that offers
only the older `VK_NV_fragment_shader_barycentric` reports false.

Nothing in `ara-gpu`'s own cohort uses barycentrics, so the proof is Renderlab's
backend suite under validation on a Vulkan device, which has not been rerun yet.
Ray-query position fetch is deliberately not enabled: hit positions come from
retained geometry (`hit_geometry`), and Renderlab's modules no longer declare the
SPIR-V extension.

## The first layer of an array as a render target (same run)

`BeginRenderingSubresources` took a shortcut for mip 0, layer 0: it attached the
image's own attachment view. On an array that view spans every layer, so rendering
to layer 0 alone also required layer 1 to be in the attachment layout; Renderlab's
per-caster transmission layers hit `VUID-vkCmdBeginRendering-pRenderingInfo-09592`
with layer 1 still `UNDEFINED`. The shortcut now applies only to images with one
layer; every other selected slice, layer 0 included, gets its own cached single-slice
view. The existing sibling-preservation probe uploads the whole image first, which
hid this. `regression_first_layer_of_an_array_renders_without_touching_its_siblings`
transitions and renders one layer at a time, layer 0 first, for a colour and for a
depth array; it passes on Metal and has not run on Vulkan, where it is expected to
fail validation without the fix. It detects the fault through validation only,
which debug builds enable. A resolve target at layer 0 of an array takes the same
view path and has no test.
