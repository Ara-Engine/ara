# ara-gpu

## Public API

- `new_raster` omits RT features/descriptors; `new` enables optional RT.
  Windowed Vulkan devices request KHR/EXT HIGH; denial retries MEDIUM.
- `Device`: portable descriptors, queues, cooked pipelines, recording and retirement.
- `DeviceInfo::ray_tracing_supported` is physical support; `ray_tracing` is logical enablement. User settings query the former.
- `Command::FillBuffer` repeats a u32 pattern over aligned ranges (zero clears).
  `UpdateBuffer` has no 64 KiB cap: large Vulkan writes use completion-gated
  staging chunks; Metal uses its pooled per-submission scratch. Updates order
  surrounding buffer accesses; fills use graph transfer barriers.
- `Command::CopyBufferRange` copies aligned ranges with checked capacities/overflow;
  it preserves unrelated destination bytes and is rejected inside rendering.
- `VulkanDevice`: Vulkan backend; `context()` is read-only native integration access; `info()`, `wait_idle()` and `host_wait_stats()` are the portable forms both backends (`GpuDevice`) provide. Unsafe startup-only `split_loading` shares native lifetime with separate pools/arenas; serialize owners if graphics queues alias.
  `prepare_resources` admits two unpublished allocation/host-copy jobs; poll/cancel
  never waits. Adoption stays on the owner, as do ordered uploads and queue access.
- `Context::host_wait_stats` snapshots cumulative host time in explicit driver waits.
  Native integration uses `measure_host_wait` for leaf waits only; never nest it.
- Timestamp pools are reused (32, at most 1024 queries); in-flight pools stay exclusive,
  overflow pools are freed on resolve. Pass begins follow leading barriers; `PassTiming`
  carries device start/end; `timing_window` spans resolved submissions and gaps (shared
  clock via calibrated timestamps, else no mixed-queue span). `set_pass_timing(false)`
  makes Metal lists untimed; Vulkan keeps timing. Tests cover ordering, overlap, wrap.
- `upload_range(buffer, byte_offset, bytes)` writes only changed coherent rows,
  with the same bounds and in-flight-use checks as `upload`.
- `create_texture(&TextureDesc)` adds volumes, arrays, mip chains on all uses and
  explicit sample counts. `UploadImageRegion`, `ReadImageRegion`, `CopyImageRegion`
  use checked texel rectangles/mips/layers. `copy_byte_len` sizes packed whole transfers,
  finest mip first; depth mip starts align to four bytes (padding is untouched).
  `sampled_view`/`storage_view` publish immutable mip/layer views; graph reachability
  and retirement follow the parent image. Storage views expose one mip.
- `Handle<T>`: arena/device identity plus a 64-bit generation. Shader `HeapIndex`
  identities are separate. An expired CPU handle never aliases a replacement.
- `Recorder` / `CommandList`: owned portable commands; no borrowed ECS state.
- Mesh/task/indirect mesh dispatch requires `DeviceInfo::mesh_shading`; ordinary
  vertex/indirect draws remain available. Mesh reads use ALL_GRAPHICS barriers.
- `Recorder::barriers` / `barrier_ranges` record simultaneous batches; byte and mip/layer
  ranges are checked. Vulkan emits one dependency call; Metal orders whole stages.
  `after: Undefined` discards; only whole-image discards suppress attachment stores.
  `TRANSIENT_ATTACHMENT` images are attachment-only: Metal memoryless, Vulkan lazily allocated.
- `RenderState::raster` adds depth compare/bias, cull/winding, per-target blend,
  sample count and explicit vertex layouts. `DrawVertexBuffers` binds up to eight
  streams, including per-instance records. `ResolveImage` resolves backed 4x color
  targets outside rendering; `BeginRenderingResolved` supports in-pass resolves and
  `ImageShape::transient` MSAA attachments. `BeginRenderingSubresources` selects
  color/depth/resolve mip and layer; Vulkan recording is pending its PC owner.
- `AccelerationDesc` selects BLAS geometry or TLAS instances. `AccelerationOptions`
  enables rebuild/refit/compaction and reserves TLAS capacity. `update_instances`
  uploads on the owning queue; rebuild permits count changes, refit preserves count.
  BLAS compaction is a blocking setup operation returning a fresh identity; compacted
  BLASes can refit, while a rebuild requires an uncompacted allocation.
- `cooked`: versioned descriptions, artifact hashes, ABI fingerprint, JSON writer.
  `resource_dir` finds executable-relative installed shaders before checkout data;
  a distribution with engine.json never silently falls back to checkout shaders.
- Development `ARA_SHADER_DIR=<workspace>` overrides cooked resources; a flat
  cohort uses `ARA_SHADER_PACKAGE`. Background cohort reload publishes stable
  handles at submission boundaries, with deferred retirement and last-good fallback.
  `VulkanDevice::{request_shader_reload,shader_epoch,shader_status}` exposes control/provenance.
- `shared` (portable): cross-process shared surfaces (D19 embedded Play). `SLOTS` = 3,
  `SurfaceDesc`/`SlotDesc`/`DeviceIdentity` (JSON, camelCase), `Frame`, `FrameReady`,
  producer `Ring`, owner `Mailbox`, `SharedSurface` (owned NT handles:
  `duplicate_from(&Child, desc)`, `duplicate_local`, `try_clone`).
- `vulkan::external`: `ExportRing` (`new`, `resize` = new generation, `publish(source,
  tick)` copies + waits, `release`, `desc`) and `ImportedSurface` (`new` checks device
  and driver UUIDs, `exchange(acquire, release)`, `image`, `read`). `supported(gpu)`.
  `DeviceInfo::{external_memory, identity}`; `GpuError::External` = `ARA_GPU_EXTERNAL`.

## Contracts

The v4 heap (v3 prefix compatible) uses runtime image/buffer arrays at bindings 5/1: up to 65,536 sampled images and 4,096 storage buffers, clamped jointly to set/stage/pool limits (minimum 128 images/64 buffers; capacity changes preserve the runtime-array ABI), plus 128 storage images and optional 16 acceleration structures.
A full storage heap waits for completed retirement (`reclaim_storage`). Vulkan sRGB
storage uses a mutable UNORM alias (shaders encode sRGB); window devices enable present scaling.
`DeviceInfo` records both selected capacities. Binding 0 mirrors the first 128 slots for cached editor
material previews only. New cooks use v4; v3 prefixes remain valid. Only material previews accept v1.
Binding 2 reserves three default samplers: material repeat (up to 16x anisotropic where the
device allows; native materials sample it with gradients), environment U repeat/V clamp
and temporal clamp (both trilinear), with native mip LODs enabled. Anisotropy affects explicit LODs too; exact trilinear readers use another sampler. `sampler(&SamplerDesc)` caches immutable descriptions in 64 slots
(defaults occupy 0..3): nearest/linear filters, Repeat/MirrorRepeat/ClampToEdge,
and up to 16x anisotropy. Regular samplers use binding 2 / DXIL s0 space0;
comparison samplers use binding 6 / s0 space5, also 64 slots. Their converter
pointers are at bytes 64/72; old v3 metallibs retain the prefix. Roots stay 32 bytes.
Declare every resource reachable through root indices in the graph.

Insertions allocate fresh slots. Resource retirement invalidates its CPU handle
immediately; allocation and descriptor reuse wait for both queue timelines.
Retention is conservative across published resources in v0. A live TLAS prevents
its BLAS from retiring. Default static acceleration structures build once; options enable repeated builds/refits. Pipelines also retire through queue completion.
Private resources suballocate: Metal automatic heaps (including AS storage), Vulkan
aligned blocks segregated by memory type, address flags and buffer/image class.
Host buffers, required-dedicated and transient/external allocations stay separate.
Retirement gates reuse on both timelines. Vulkan image ranges return from disposal
only after native destruction; generation IDs reject late worker replies. Preparation
jobs transfer blocks/heaps on adoption. Metal views need explicit residency too.
Vulkan caches at most 64 MiB of empty blocks, plus the bounded host-buffer cache.

Recording validates generations, queue classes, rendering scopes, attachment
formats, copy capacities and vertex/index usage before touching native commands.
`ClearDepth` clears one rectangle of the scope's depth attachment (an atlas tile) and must lie within the scope's extent.
Native presentation and raw ash integration remain unsafe, with documented
lifetime/layout requirements. Never reset a pending native command buffer.

Interactive Scene frames can use `begin_deferred_completion` / `end_deferred_completion`:
retain the returned `Completion`, poll it before reusing resources, and keep showing
the last completed color/ID pair. `FRAMES_IN_FLIGHT` (2) ended scopes may stay unconsumed; a third begin is refused. `upload_queued`
writes busy constants through ordered GPU updates; ordinary `upload` still rejects
overwrites. A second graphics-family queue isolates Scene work from the native UI
queue where available; completion publishes a GPU memory dependency to that UI queue.
Capture/shutdown may use `wait_completion`. Tests hold a queue only through `vulkan::QueueGate` (a host
timeline wait); device teardown waits until no gate is held, as NVIDIA's Windows `vkDestroyDevice` otherwise deadlocks
under its process-wide lock (`ara-render` AGENTS.md). `tests/deferred.rs` blocks Scene work behind one: UI progress, polling, ordered updates.
Synchronous presenters use `begin_deferred_completion_on_current_queue`; consecutive
helpers share one native submit with ordered memory barriers. End the scope before
host waits. `wait_completion_with` preserves logical pass groups; `submission_stats`
counts native batches. Failed native submits drain/retire and poison further admission.

Debug creation requires Khronos validation and synchronization validation, unless
`ARA_NO_GPU_VALIDATION` is non-empty (the `ara-dev live` loop sets it; tests never do).
`assert_clean` rejects warnings and errors; `diagnostics` returns stable codes,
layer identifiers, severity and message. The diagnostic callback mutex protects
messages only, never frame resources or gameplay state.
Loader/overlay GENERAL notices are informational and emitted once per exact
message/process, with a stable bus key across child processes. Mixed GENERAL plus
VALIDATION/PERFORMANCE messages retain their original severity and counters.

Shared surfaces: `VK_KHR_external_memory_win32` is enabled whenever offered (the
editor's device must import without knowing about Play). Opaque NT handles,
unnamed; owners pull them with `DuplicateHandle` from their child. Both sides create
identical images (RGBA8, TRANSFER_SRC|DST|SAMPLED, dedicated allocation, the
producer's memory type and size). Between owners slots are owned by
`QUEUE_FAMILY_EXTERNAL`: producer → owner transfers carry TRANSFER_DST→SRC (the
validation layer only learns external layouts from a transition), owner → producer
transfers SRC→SRC; each side waits on a fence before notifying the other. A slot is
written only after the owner released that exact frame (generation, slot, serial).
Metal: global IOSurfaces (`metal::external`, private copy queue, same-session trust). Both backends share `create`/`recreate`/`publish_image`/`read_pixels`.

`prepare_pipelines` admits two PSO batches (running/queued/ready); poll/cancel never waits. One compiler owns a private persisted cache.
Caches check vendor/device/UUID/driver and save atomically; cooked artifact and layout hashes are checked separately.
Adopt ready pipeline handles on the owner and retire normally. Keep the last presentation or skip draws until ready.

## Verification

Set `CARGO_BUILD_JOBS=2`. All gates: `../ara-render/run.ps1 -Mode Check`; or `cargo check`, `test`,
`clippy --all-targets -- -D warnings` and `fmt -- --check`, each with `-p ara-gpu`.
Vulkan GPU tests run on the PC; all Mac-written Vulkan changes remain unverified.
Metal gaps tests use native transfers and MSL probes; fresh converter artifacts are cooked on Windows (docs/dev/macos.md).
Metal 4 (macOS; `src/metal/mod.rs` holds the contract). Portable aliases: `GpuDevice`, `GpuCompletion`, `NativeImage`, `NativeView`, `external`.
Converter ABI (`abi.rs`) keeps the MSC 4.0.3 prefix and appends comparison samplers; one MTL4Compiler + archive serializer per process (`cache.rs`, flushed once, never under shader validation); one timestamp heap per device, shared as slot ranges (`timestamps.rs`; 32 heaps per process), with back-to-back pass boundaries sharing one command-buffer timestamp (each is a driver dispatch); queue barriers carry their accesses' stages (`encode.rs`), so an attachment-only dependency waits in the fragment stage and the pass's tiling overlaps the previous pass; a deferred scope's lists commit as one batch per queue (`flush`), like Vulkan; `native::pool` around Metal calls.
Presentation and shared-surface copies blit, or scale/convert with MPS, on a classic queue (`transfer.rs`); a stalled present (pending copy, full drawable pool) skips after `PRESENT_WAIT`, never blocking the pump; `adopt_texture` shares textures across devices of the GPU (UI adapter, loading split). RT tier (`new`, Apple9+; `acceleration.rs`): BLAS/TLAS built/refit on a compute encoder, all resident (a TLAS names BLASes by ID); a bound TLAS is a descriptor holding the address of MSC's 64-byte AS header (resource ID, per-instance hit contributions), which cooked `RayQuery` stages read with no extra converter flags.
Test: `MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --lib metal`, and once without shader validation; Vulkan GPU tests are ignored on macOS.
