# Ara M1 renderer baseline

`ara-gpu` owns native resources, bindless indices, queues and retirement. `ara-render`
owns the graph, PBR passes and visual feedback API. `ara-platform` owns SDL3
window/input and native presentation; `ara-shader-cook` is an offline executable.

From the repository root on Windows, run `crates/ara-render/run.ps1 -Mode Check`.
SDL builds statically with `cc`; no setup script or SDL3.dll is required.
Use `-Mode Capture` for all debug
PNGs/marks/legends/ID channels and `-Mode Window -Frames 120` for the live demo.
The Vulkan SDK 1.4.363.0 is needed to recook, not to run cooked shaders:
`cargo run -p ara-shader-cook -- shaders/pipelines.json crates/ara-render/cooked`.

`Renderer::capture(CaptureRequest)` accepts arbitrary camera poses or bounds,
pixel size, view mode, exposure, fixed tick/seed and settle frames. Determinism
resets history and disables jitter/history blending. `Capture::entity_at` returns
a stable u64 entity identity; marks use visible-pixel bounds, including transparent
objects. Metadata records scene/shader hashes, compiler, camera and device/driver.
`compare` returns local RGB 1-SSIM, a tolerance verdict and a 4x amplified diff.

Coordinates are right-handed and Y-up, in metres. Camera depth is infinite-far
reversed-Z with a 0.1 m near plane. The shadow atlas has four 512x512 cascades
(8/18/40/100 m splits) and 3x3 PCF. Opaque geometry uses a five-target G-buffer;
16x16 light tiles hold up to 64 checked point/spot lights. Transparent objects
are sorted back-to-front, shaded forward and included in the diagnostic channels.

The metallic/roughness model uses GGX, Smith/Schlick visibility, tangent normal
maps and emissive radiance. Directional input is lux; local input is lumens with
inverse-square falloff and a finite-radius fade. HDR inputs are linear radiance
in cd/m?. Compute passes integrate cosine-weighted diffuse irradiance and five
GGX specular roughness levels (1024 samples per texel), with an analytic split-sum
BRDF fit. The example calibrates its relative HDR source by multiplying by 500.

TAA uses an eight-sample Halton jitter, previous camera projection, depth
rejection and 3x3 neighborhood clamping. Captures bypass temporal blending.
Exposure is a manual linear multiplier. The SDR tone mapper is the ACES fitted
approximation `clamp((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14), 0, 1)`, after
exposure, followed by the standard piecewise sRGB transfer function. This is an
ACES-shaped fit, not the complete ACES color-management pipeline or HDR display output.

GPU timestamps below are from the RTX 4070 SUPER, Vulkan 1.4.351 driver
2588114944, 1280x720, debug + synchronization validation, live TAA, 120 samples
after eight warmup frames. Frames are serialized for inspectability. Tile culling
uses compute family 2; graphics uses family 0, with explicit paired ownership
barriers and timeline waits. Pass times are not a measurement of queue overlap.

| Pass | p50 ms | p95 ms |
|---|---:|---:|
| ShadowAtlas.4Cascades | 0.029 | 0.036 |
| GBuffer | 0.144 | 0.150 |
| TiledLightCull | 0.008 | 0.018 |
| DeferredLighting | 0.175 | 0.189 |
| ForwardTransparency | 0.026 | 0.028 |
| TemporalResolve | 0.104 | 0.110 |
| Exposure.ACESFitted.SDR | 0.058 | 0.062 |
| CaptureReadback | 0.278 | 0.278 |
| HostReadback | 0.003 | 0.003 |
| TileOwnershipReturn | 0.000 | 0.000 |

IBL integration is initialization-only: approximately 0.4 ms irradiance and 7?9 ms
specular prefilter on warm runs; cold/low-clock runs were slower. Full measured
provenance is in [measurements/gpu-timings.json](measurements/gpu-timings.json).
The [before measurement](measurements/gpu-timings-before-device-local.json) records
the host-visible static-buffer version. Static geometry, materials and lights now
use device-local memory with startup staging transfers on their owning queue;
only frame uploads/readbacks remain host-coherent. GPU clocks were uncontrolled,
so these samples document the change without establishing a frame-time speedup.

Verified engine PNGs: lit, albedo, normals, reversed depth, roughness/metal,
entity IDs, cascade visibility, and numbered marks. Seven committed goldens pass
1-SSIM <= 0.005, with repeated deterministic captures byte-identical. The SDL
Vulkan presentation path ran 120 frames and resized 1280x720 -> 960x540 with zero
validation warnings/errors. Native desktop screenshot inspection was unavailable
because the computer-use helper pipe was absent; image inspection used GPU readbacks.

The example loads the real glTF through the `gltf` crate in `examples/common`,
with procedural spheres, an emissive, transparency, three local lights and sun.
[Avocado](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Avocado)
is Microsoft CC0; [Studio Small 09](https://polyhaven.com/a/studio_small_09) is
Sergej Majboroda / Poly Haven CC0. Original notices and source URLs accompany the media.

M3 adds the material AST, animated object velocities, richer GI/reflections,
shadow fidelity, streaming and memory aliasing/suballocation. Scene extraction
is immutable in this baseline; gameplay and production asset import remain the
host/asset pipeline?s responsibility. The cfg-gated Metal backend is the Mac
session?s implementation point for Metal 4 and converter-generated metallibs.
