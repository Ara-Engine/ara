# GPU VFX

`GpuSystem` owns particle resources on an engine-owned `VulkanDevice`. The engine
can retain several systems, or render a headless preview using a fresh device.
There is no compiler or build script in the runtime dependency chain.

The runtime registers explicit `ara-render::graph` passes for prepare, spawn,
update, scan, group prefix, scatter, GPU events, sort, indirect arguments, draw,
tone mapping and readback. The particle buffers are device-local SoA channels:
position/age, velocity/lifetime, color and diameter/seed/birth/event flags.
Stable alive/dead indices use parallel group scans; a second parallel scan of
contiguous group chunks preserves slot order. There are no unordered appends.

Each emitter has a power-of-two physical partition and a checked logical
capacity. The system cap partitions capacity in author order. Projected coverage
is accumulated with the scan; a stable prefix bounds rendered particles under
the effect's overdraw budget, distributed across emitter capacities. Simulation
continues for particles omitted from drawing. Sorting applies to the selected
draw list, with slot IDs breaking ties. Bounds culling omits whole batches.

Event positions are copied before slot reuse, compacted and routed entirely on
the GPU, then consumed at the next tick. Quality caps apply before writes. GPU
events are cosmetic. Gameplay-critical effects use `ara_vfx::CpuSystem`, a Bevy
ECS component with authoritative events and the same fixed-tick modules.

## Native renderer integration

Create the system at a completed frame boundary using `renderer.gpu_mut()` and
an offline cooked directory. Render the scene, then import its HDR color/depth:

```rust,ignore
let (image, sampled_index, state) = renderer.feature_depth();
let scene_depth = SceneDepth { image, sampled_index, state };
let (image, sampled_index, state) = renderer.feature_color();
let scene_color = SceneColor { image, sampled_index, state };
let view = View {
    camera,
    scene_depth: Some(scene_depth),
    scene_color: Some(scene_color),
    ..View::default()
};
system.step(renderer.gpu_mut(), &view)?;
let display_image = system.output();
```

This preserves scene HDR content beneath premultiplied particles. Present the
feature's SDR output. Imported targets must match the feature's extent and
device; HDR is RGBA32 float and depth is reversed-Z. They return in
Graphics/ShaderRead. The integration test renders again afterward to verify
that the baseline renderer can continue using both targets. Omitting the
imports creates the deterministic grid preview. Call `retire` when removing a
system. Resource lifetime follows GPU queue completion.

The native stack panel consumes `ara_vfx::Stacks::STAGES`, library search metadata
and the typed module variants. Use `vfx.text.apply`, `vfx.stack.edit` and
`vfx.parameter.set` through the shared registry; every edit produces undoable
kernel ops. Compile the changed effect in a worker below normal priority, retain
the last good cohort on errors, and publish only at a permitted frame boundary.
No editor shell or document side-channel writes are needed.

## Materials, geometry and lights

`cook_source` incorporates the actual `ara-material` generator and engine math.
Sprite, ribbon and mesh outputs accept surface, unlit or UI domains. Surface
output uses the shared GGX lighting evaluator; unlit/UI output stays unlit.
The preview supplies a directional light and capped emitter point lights.
Runtime lit-particle scene-light/probe extraction can feed that same evaluator;
the baseline scene's full light/probe tables are not yet imported automatically.

Pass one checked material per emitter to `cook_source` for custom shading.
`Instance::pack` produces typed float4 rows; `bind_materials` accepts up to 64 rows
per emitter plus every sampled image reachable by those rows. Texture heap
resolution belongs to the asset renderer. The material crate currently restricts
packed texture indices to its first 16 slots. Bind only the matching cooked
material layout. Ordinary row changes do not require new shader variants.

Flipbook UVs use grid dimensions and particle age; an atlas material receives
the resulting coordinates. Ribbons connect particles from oldest to newest.
Mesh particles consume cooked local triangles and orient to velocity. Mesh
surface initialization samples cooked world triangles by area. Soft particles
use reconstructed depth distance. GPU depth collision reconstructs a surface
normal from depth neighbors; CPU gameplay needs an explicit `DepthQuery`.

## Reproduce captures and timings

From the repository root on Windows, with `CARGO_BUILD_JOBS=2`:

```powershell
./crates/ara-vfx-gpu/run.ps1 -Mode Check
./crates/ara-vfx-gpu/run.ps1 -Mode Capture
./crates/ara-vfx-gpu/run.ps1 -Mode Timings
```

`run.ps1` scopes the empty implicit-layer directory, validation layer path and
SDL/Vulkan PATH entries to its process. It restores the prior environment.
Set `VULKAN_SDK=C:\VulkanSDK\1.4.363.0` if SDK discovery needs an explicit path.
Only `-Mode Cook` invokes external Slang/SPIR-V validation, producing committed
SPIR-V, DXIL, reflection and content-keyed pipeline manifests.

`GpuSystem::capture` resets simulation and replays exactly to the requested
completed tick. Extra settle frames redraw without advancing time. Capture seed
must match the authored seed. It uses `ara-render::Capture::from_feature`, with
effect IDs from an actual GPU coverage channel, camera/tick/seed, effect hash,
compiler/layout key, GPU/driver provenance and measured pass timestamps.
The capture API currently exposes the lit channel. `read_output` captures an
existing view without resetting or stepping it.

Six committed PNG goldens cover campfire, plane-colliding sparks, a curl/vortex
swirl, lit smoke, a muzzle flash with a light, and rain with GPU splash events.
Tests compare independent replays byte-for-byte, compare against the PNG goldens
with 1-SSIM <= 0.005, check CPU/GPU counts and sampled particle state, and inspect
visible feature bounds. Separate tests exercise ribbons, mesh surfaces/geometry,
depth collisions, frozen settling and composition into the merged renderer.
Goldens update only under explicit `ARA_UPDATE_GOLDENS=1`.

`captures/*.stats.json` reports alive/dead/rendered counts, event counts, dropped
work, projected overdraw, batches, lights and actual per-stage GPU milliseconds.
`captures/timings.json` records 30 warmup frames and 120 samples for 1M updated
and drawn sprites at 2560x1440, on both graphics and dedicated compute queues.
The benchmark fails if either live or rendered count is less than 1M. Stage
timestamps exclude intervening barriers/submission overhead; they are not a
whole-frame wall-time claim. No Niagara-equivalent baseline has been measured.
Same-device GPU replay is exact; cross-vendor floating-point identity is not
promised. Async compute stays opt-in because this workload offers little overlap.
