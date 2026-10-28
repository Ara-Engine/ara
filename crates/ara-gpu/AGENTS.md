# ara-gpu

Owns the native GPU boundary. Vulkan uses ash revision
`f4c2ca3e4f6b998d5254ad101a32f024d87cdec2`, matching S1.

## Public API

- `VulkanDevice::new_raster` creates a raster-only logical device: no ray-query/AS
  extensions, features or acceleration descriptor capacity, even on an RT GPU.
  `new` retains optional RT. Windowed devices request KHR/EXT HIGH; denial retries MEDIUM.
- `Device`: portable descriptors, queues, cooked pipelines, recording and retirement.
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
- `VulkanDevice::create_mip_image`: native sampled mip chains, including BC4/5/7.
  `UploadImage`/`ReadImage` copy every resident level, finest first, without padding
  between levels. Barriers span the chain; copy validation checks block footprints.
- `Handle<T>`: arena/device identity plus a 64-bit generation. Shader `HeapIndex`
  identities are separate. An expired CPU handle never aliases a replacement.
- `Recorder` / `CommandList`: owned portable commands; no borrowed ECS state.
- `DeviceInfo::{mesh_shading,buffer_int64_atomics}`: queried optional tiers.
  `Stage::{Mesh,Task}` and `DrawMeshTasksIndirect` require the mesh tier; the
  existing vertex/indirect path remains available. Mesh reads use ALL_GRAPHICS
  barriers. Slang PrimitiveId also needs the reported core geometryShader feature.
- `Barrier`, `Access`, `Handoff`, `TimelinePoint`: graph synchronization; `after: Undefined` discards, so `attachments::ops` skips the scope's store and colour load.
  `TRANSIENT_ATTACHMENT` images are attachment-only: Metal memoryless, Vulkan lazily allocated.
- `AccelerationDesc`: static BLAS/TLAS allocation; build with a typed command.
  `TriangleRange` selects checked aligned byte ranges in shared vertex/index
  allocations; indices remain local to the range. Multiple BLAS builds and their
  transformed TLAS instances can share one upload and submission.
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

The v3 heap uses runtime image/buffer arrays at bindings 5/1: up to 65,536 sampled
images and 128 storage buffers, clamped jointly to set/stage/pool limits (minimum
128 images/64 buffers), plus 64 storage images and optional 16 acceleration structures.
`DeviceInfo` records both selected capacities. Binding 0 mirrors
the first 128 slots for cached editor material previews only. All engine pipelines
must be recooked; only `previewVertex`/`previewFragment` caches accept the old ABI.
Binding 2 has three fixed trilinear samplers: material repeat (up to 16x anisotropic
where the device allows; native materials sample it with gradients), environment U
repeat/V clamp, and temporal clamp, with native mip LODs enabled. Root constants are 32 bytes.
Declare every resource reachable through root indices in the graph.

Insertions allocate fresh slots. Resource retirement invalidates its CPU handle
immediately; allocation and descriptor reuse wait for both queue timelines.
Retention is conservative across published resources in v0. A live TLAS prevents
its BLAS from retiring. Static acceleration structures build once; replacement
uses a fresh identity. Pipelines also retire through queue completion.
Completed images/pipelines release identities on the owner; their destruction and
uncached buffer-memory frees run on one worker (one active/one queued batch).
Backpressure retains completed objects locally; device teardown drains and joins.
Retired buffers destroy their VkBuffer but may retain up to 64 MiB/256 idle memory
blocks. Reuse requires exact size, memory type and device-address allocation flags;
required dedicated allocations are excluded. Fresh buffers keep queue ownership
and handle generations independent; images and shared surfaces do not use this pool.

Recording validates generations, queue classes, rendering scopes, attachment
formats, copy capacities and vertex/index usage before touching native commands.
`ClearDepth` clears one rectangle of the scope's depth attachment (an atlas tile)
and must lie within the scope's extent.
Native presentation and raw ash integration remain unsafe, with documented
lifetime/layout requirements. Never reset a pending native command buffer.

Interactive Scene frames can use `begin_deferred_completion` / `end_deferred_completion`:
retain the returned `Completion`, poll it before reusing resources, and keep showing
the last completed color/ID pair. One frame is admitted at a time. `upload_queued`
writes busy constants through ordered GPU updates; ordinary `upload` still rejects
overwrites. A second graphics-family queue isolates Scene work from the native UI
queue where available; completion publishes a GPU memory dependency to that UI queue.
Capture/shutdown may use `wait_completion`. `tests/deferred.rs` blocks Scene work
behind a host gate and checks UI progress, polling and byte-exact ordered updates.
Synchronous presenters use `begin_deferred_completion_on_current_queue`; consecutive
helpers share one native submit with ordered memory barriers. End the scope before
host waits. `wait_completion_with` preserves logical pass groups; `submission_stats`
counts native batches. Failed native submits drain/retire and poison further admission.

Debug creation requires Khronos validation and synchronization validation.
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

Optional `tracy` enables literal `cpu_scope!` zones for submission, recording and
timeline waits. The executable starts the client (`ara-player --features tracy`);
without a running client library zones do nothing. Default builds erase them.

Set `CARGO_BUILD_JOBS=2`. Use `../ara-render/run.ps1 -Mode Check` for all gates,
or `cargo check -p ara-gpu`, `cargo test -p ara-gpu`,
`cargo clippy -p ara-gpu --all-targets -- -D warnings`,
`cargo fmt -p ara-gpu -- --check`.
GPU tests need Vulkan 1.3, validation layers, a dedicated compute family and
process-local `VK_IMPLICIT_LAYER_PATH` pointing at an empty directory on this PC.
They verify malformed-copy rejection, stale handles and real BLAS/TLAS lifetime.
The triangle-range probe checks pooled geometry, overflow/alignment rejection
and real rays hitting three transformed instances of two shared BLAS identities.
The sampled-heap probe reads 257 nonuniform slots on GPU, publishes an unused slot
before completion, and checks native mip interpolation. Explicit offline fixture
updates: `cargo run -p ara-gpu --example heap_probe_manifest`, then
`ara-shader-cook crates/ara-gpu/tests/shaders/pipelines.json crates/ara-gpu/tests/cooked`.
`tests/external.rs` (Windows) exports a ring on one device, imports it on a second,
cycles every slot through a resize and checks pixels and validation on both.
Metal 4 (macOS; `src/metal/mod.rs` holds the contract). Portable aliases: `GpuDevice`, `GpuCompletion`, `NativeImage`, `NativeView`, `external`.
Converter ABI checked against MSC 4.0.3 (`abi.rs`: constants, four tables, then the static-sampler table); one MTL4Compiler + archive serializer per process (`cache.rs`, flushed once, never under shader validation); one timestamp heap per device, shared as slot ranges (`timestamps.rs`; 32 heaps per process), with back-to-back pass boundaries sharing one command-buffer timestamp (each is a driver dispatch); queue barriers carry their accesses' stages (`encode.rs`), so an attachment-only dependency waits in the fragment stage and the pass's tiling overlaps the previous pass; a deferred scope's lists commit as one batch per queue (`flush`), like Vulkan; `native::pool` around Metal calls.
Presentation and shared-surface copies blit, or scale/convert with MPS, on a classic queue (`transfer.rs`); `adopt_texture` shares textures across devices of the GPU (UI adapter, loading split). RT tier (`new`, Apple9+; `acceleration.rs`): static BLAS/TLAS built on a compute encoder, all resident (a TLAS names BLASes by ID); a bound TLAS is a descriptor holding the address of MSC's 64-byte AS header (resource ID, per-instance hit contributions), which cooked `RayQuery` stages read with no extra converter flags.
Test: `MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 cargo test -p ara-gpu --lib metal`, and once without shader validation; Vulkan GPU tests are ignored on macOS.
