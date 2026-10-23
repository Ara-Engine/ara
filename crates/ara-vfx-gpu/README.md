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
`View::lighting` supplies the scene sun, ambient fallback and an optional completed
fidelity DDGI field. Lit sprites sample the actual directional probes. Their baked
density gradients shape the normal; Beer-Lambert opacity approximates local self
shadowing. This is a sprite approximation, without inter-particle volume shadows.

`FidelityView::feature_inputs` publishes read-only HDR, positive linear depth and
probe storage. Set `scene_background`, `scene_depth`, `linear_depth` and the
lighting inputs, then redraw the completed VFX tick with `GpuSystem::render`.
Pass `hdr_output()` into `FidelityView::post_process` to run the shared bloom,
exposure, lens and display pipelines. It resets display history for composition.
The gallery extracts capped emitter lights into the actual scene, with fixed-tick
fire flicker and a muzzle-flash envelope lighting the floor and bronze prop.

Pass one checked material per emitter to `cook_source` for custom shading.
`Instance::pack` produces typed float4 rows; `bind_materials` accepts up to 64 rows
per emitter plus every sampled image reachable by those rows. Texture heap
resolution belongs to the asset renderer. The material crate currently restricts
packed texture indices to its first 16 slots. Bind only the matching cooked
material layout. Ordinary row changes do not require new shader variants.

The typed `appearance` module selects fire, smoke, spark, ripple or wisp sheets,
alpha/additive output, camera/velocity/ground alignment, stretch and a shutter.
`cook_atlas` creates original noise-based RGBA8 assets offline. Smoke integrates a
3D density field into each frame. Adjacent built-in frames blend over normalized
particle life; the separate `flipbook` UV module supplies grid/FPS addressing to
custom atlas materials. No texture synthesis runs during ordinary builds or play.
The atlas is `cooked/atlas.rgba`; `atlas.png` visualizes density/lighting channels.

Alpha output uses premultiplied source-over. Additive output carries premultiplied
radiance with zero transmittance alpha, so the same native blend state adds light.
A separate unblended GPU coverage attachment retains correct picking/capture IDs.
Velocity sprites cover the fixed-tick shutter path with a tapered exposure mask.
Ground-aligned ring sheets form splash ripples. Ribbons join oldest to newest.
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

Six committed still/sequence PNG golden pairs cover campfire, plane-colliding
sparks, a curl/vortex swirl, lit smoke, a muzzle flash with a light, and rain with
GPU splash events. The gallery uses a lit ground, props, RT shadows, probes and
fidelity bloom. Each frame independently replays its tick and converges 64 fixed
scene frames. `captures/sequences/` contains every full-size sequence frame;
`captures/*.sequence.json` records the four strip ticks. Main PNGs are mid-life.
Tests compare independent replays byte-for-byte, compare against the PNG goldens
with 1-SSIM <= 0.005, check CPU/GPU counts and sampled particle state, and inspect
visible feature bounds. Separate tests exercise ribbons, mesh surfaces/geometry,
depth collisions, frozen settling and composition into the merged renderer.
Goldens update only under explicit `ARA_UPDATE_GOLDENS=1`.

`captures/*.stats.json` reports alive/dead/rendered counts, event counts, dropped
work, projected overdraw, batches, lights and actual per-stage GPU milliseconds.
`captures/timings.json` records 30 warmup frames and 120 samples for 1M updated
and drawn sprites at 2560x1440, at both 0.003 m and 0.05 m diameters, on graphics
and dedicated compute queues. Both sizes use identical motion, camera and a
6x3.6x1.6 m box entirely above ground and inside the camera. Coverage is reported
explicitly; the 64-layer budget must retain every particle for a valid run.
The benchmark fails if either live or rendered count is less than 1M. Stage
timestamps exclude intervening barriers/submission overhead; they are not a
whole-frame wall-time claim. [COMPARISON.md](COMPARISON.md) records the nearest
published Niagara/VFX Graph numbers and the differences preventing a speedup claim.
Same-device GPU replay is exact; cross-vendor floating-point identity is not
promised. Async compute stays opt-in because this workload offers little overlap.

Measured on the RTX 4070 SUPER with Vulkan SDK 1.4.363.0 after merging main
through `41cbc25` (2026-10-23), in ms:

| Stage, median | 0.003 m graphics | 0.003 m compute | 0.05 m graphics | 0.05 m compute |
|---|---:|---:|---:|---:|
| Update | 0.279552 | 0.277504 | 0.280576 | 0.277504 |
| Compact scan | 0.132096 | 0.131072 | 0.133120 | 0.132096 |
| Compact prefix | 0.018432 | 0.017408 | 0.018432 | 0.017408 |
| Compact scatter | 0.107520 | 0.106496 | 0.107520 | 0.106496 |
| Draw | 0.616096 | 0.560000 | 10.858176 | 10.860064 |
| Tone map | 0.112640 | 0.111616 | 0.072704 | 0.072704 |
| Sum of all stage medians | 1.406976 | 1.337568 | 11.610816 | 11.604384 |
| Estimated full-screen coverage layers | 0.2712673 | 0.2712673 | 36.044125 | 36.044125 |

The raw report includes every stage and its p95. All four runs update and submit
every one of the 1,000,000 particles. The larger footprint makes blending/fill
dominate; the small-sprite result does not predict dense smoke or fire cost.
Dedicated compute remains opt-in: these isolated stages do not establish overlap
with a game's graphics work. `timings.shared-gpu.json` preserves an earlier run
affected by another agent's active path tracer; the final report was repeated
after that process exited. Earlier optimization reports retain the prefix-scan
change from 0.423 ms to about 0.018 ms under the previous box/camera settings.

[QUALITY.md](QUALITY.md) records the visual inspection and remaining limits.
These effects improve the starter baseline but do not yet match Niagara showcase
art: visible sprite layering and repeated silhouettes still need artist work.

Verification on 2026-10-23: `run.ps1 -Mode Check` passed all-targets check, 35 tests
with GPU gates enabled, clippy with warnings denied, and formatting for `ara-vfx`,
`ara-vfx-gpu`, `ara-gpu`, `ara-render`, `ara-render-fidelity` and `ara-json`.
The twelve gallery goldens passed without update mode, including independent
byte-exact mid-life replay and CPU/GPU state comparisons. The additional ribbon,
mesh and depth fixtures were also captured and inspected.

The canonical float migration additionally passed check/clippy on all targets,
175 tests, and formatting for `ara-schema`, `ara-cmd`, `ara-graph`, `ara-material`,
`ara-anim`, `ara-asset`, `ara-audio`, `ara-foliage`, `ara-terrain`, `ara-world`,
`ara-nav`, `ara-ui`, `ara-ui-test`, `ara-importers`, `ara-pack` and `ara-export`.
Four existing opt-in/documentation tests remained ignored (Steam Audio runtime,
prepared export/signing toolchains, and the schema documentation example).
