# ara-gpu

Owns the native GPU boundary. Vulkan uses ash revision
`f4c2ca3e4f6b998d5254ad101a32f024d87cdec2`, matching S1.

## Public API

- `Device`: portable descriptors, queues, cooked pipelines, recording and retirement.
- `VulkanDevice`: Vulkan backend; `context()` is read-only native integration access.
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
- `Barrier`, `Access`, `Handoff`, `TimelinePoint`: explicit graph synchronization.
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

The v2 global update-after-bind heap has a variable-count runtime sampled-image
array at binding 5 (up to 65,536 slots, clamped to queried set/stage/pool limits),
64 storage buffers, 64 storage images and, on RT devices, 16 acceleration structures.
`DeviceInfo.sampled_image_capacity` records the selected capacity. Binding 0 mirrors
the first 128 slots for cached editor material previews only. All engine pipelines
must be recooked; only `previewVertex`/`previewFragment` caches accept the old ABI.
Binding 2 has three fixed trilinear samplers: material repeat, environment U
repeat/V clamp, and temporal clamp, with native mip LODs enabled. Root constants are 32 bytes.
Declare every resource reachable through root indices in the graph.

Insertions allocate fresh slots. Resource retirement invalidates its CPU handle
immediately; allocation and descriptor reuse wait for both queue timelines.
Retention is conservative across published resources in v0. A live TLAS prevents
its BLAS from retiring. Static acceleration structures build once; replacement
uses a fresh identity. Pipelines also retire through queue completion.
Retired buffers destroy their VkBuffer but may retain up to 64 MiB/256 idle memory
blocks. Reuse requires exact size, memory type and device-address allocation flags;
required dedicated allocations are excluded. Fresh buffers keep queue ownership
and handle generations independent; images and shared surfaces do not use this pool.

Recording validates generations, queue classes, rendering scopes, attachment
formats, copy capacities and vertex/index usage before touching native commands.
Native presentation and raw ash integration remain unsafe, with documented
lifetime/layout requirements. Never reset a pending native command buffer.

Debug creation requires Khronos validation and synchronization validation.
`assert_clean` rejects warnings and errors; `diagnostics` returns stable codes,
layer identifiers, severity and message. The diagnostic callback mutex protects
messages only, never frame resources or gameplay state.

Shared surfaces: `VK_KHR_external_memory_win32` is enabled whenever offered (the
editor's device must import without knowing about Play). Opaque NT handles,
unnamed; owners pull them with `DuplicateHandle` from their child. Both sides create
identical images (RGBA8, TRANSFER_SRC|DST|SAMPLED, dedicated allocation, the
producer's memory type and size). Between owners slots are owned by
`QUEUE_FAMILY_EXTERNAL`: producer → owner transfers carry TRANSFER_DST→SRC (the
validation layer only learns external layouts from a transition), owner → producer
transfers SRC→SRC; each side waits on a fence before notifying the other. A slot is
written only after the owner released that exact frame (generation, slot, serial).
Metal: unsupported (error).

Pipeline caches validate vendor/device/UUID headers and are keyed by UUID,
vendor/device and driver; saves replace the file atomically. Cooked artifact hashes
and layout hashes are checked separately before pipeline creation.

## Verification

Optional `tracy` enables literal `cpu_scope!` zones for submission, recording and
timeline waits. The executable starts the client (`ara-player --features tracy`);
without a running client library zones do nothing. Default builds erase them.

Set `CARGO_BUILD_JOBS=4`. Use `../ara-render/run.ps1 -Mode Check` for all gates,
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

Metal 4's implementation point is `metal.rs`, enabled only on macOS. Implement
this same portable surface using converter-compatible root buffers, argument
tables, residency and completion retirement; do not emulate Vulkan heap packing.
