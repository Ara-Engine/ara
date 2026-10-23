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
- `cooked`: versioned descriptions, artifact hashes, ABI fingerprint, JSON writer.
  `resource_dir` finds executable-relative installed shaders before checkout data;
  a distribution with engine.json never silently falls back to checkout shaders.

## Contracts

The v1 global update-after-bind heap has 128 sampled images, 64 storage buffers,
64 storage images and, on RT-capable devices, 16 acceleration structures.
Binding 2 has three immutable linear samplers: material repeat, environment U
repeat/V clamp, and temporal clamp. Root constants are exactly 32 bytes.
Declare every resource reachable through root indices in the graph.

Insertions allocate fresh slots. Resource retirement invalidates its CPU handle
immediately; allocation and descriptor reuse wait for both queue timelines.
Retention is conservative across published resources in v0. A live TLAS prevents
its BLAS from retiring. Static acceleration structures build once; replacement
uses a fresh identity. Pipelines also retire through queue completion.

Recording validates generations, queue classes, rendering scopes, attachment
formats, copy capacities and vertex/index usage before touching native commands.
Native presentation and raw ash integration remain unsafe, with documented
lifetime/layout requirements. Never reset a pending native command buffer.

Debug creation requires Khronos validation and synchronization validation.
`assert_clean` rejects warnings and errors; `diagnostics` returns stable codes,
layer identifiers, severity and message. The diagnostic callback mutex protects
messages only, never frame resources or gameplay state.

Pipeline caches validate vendor/device/UUID headers and are keyed by UUID,
vendor/device and driver; saves replace the file atomically. Cooked artifact hashes
and layout hashes are checked separately before pipeline creation.

## Verification

Set `CARGO_BUILD_JOBS=4`. Use `../ara-render/run.ps1 -Mode Check` for all gates,
or `cargo check -p ara-gpu`, `cargo test -p ara-gpu`,
`cargo clippy -p ara-gpu --all-targets -- -D warnings`,
`cargo fmt -p ara-gpu -- --check`.
GPU tests need Vulkan 1.3, validation layers, a dedicated compute family and
process-local `VK_IMPLICIT_LAYER_PATH` pointing at an empty directory on this PC.
They verify malformed-copy rejection, stale handles and real BLAS/TLAS lifetime.

Metal 4's implementation point is `metal.rs`, enabled only on macOS. Implement
this same portable surface using converter-compatible root buffers, argument
tables, residency and completion retirement; do not emulate Vulkan heap packing.
