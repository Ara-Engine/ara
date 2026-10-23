# Fidelity view

`FidelityView` is an opt-in graph extension; the M1 renderer remains available.
It borrows `VulkanDevice` only while allocating/updating/recording, owns per-view
histories and targets, and returns a completed offscreen image. The only baseline
change is `Renderer::gpu_mut()` for frame-boundary extension work.

```rust,ignore
let mut view = FidelityView::new(renderer.gpu_mut(), &snapshot, settings, None)?;
view.render(renderer.gpu_mut(), &frame_input)?;
let image = view.output(); // RGBA8 SDR, CopySource; also sampled/storage/color capable
// Editor outlines/gizmos/compositors may import it, then restore CopySource.
view.destroy(renderer.gpu_mut())?;
```

`resize` replaces view targets and resets histories, preserving scene/AS/probes.
`feature_inputs()` publishes the completed filtered HDR, positive view-space
depth and directional probe storage with their resource states. Borrow these at
a completed frame boundary to shade/composite an external feature. Pass its
matching-extent HDR `ImageInput` to `post_process()` to run this view's actual
bloom, exposure, lens, display and upscale pipelines over the composite. The
external image returns in Graphics/ShaderRead, display history resets, and the
SDR output remains CopySource. This seam does not advance scene transport.

`update_scene` changes extraction at a completed frame boundary, preserving GPU
IDs and unchanged BLASes. Position-only deformations retain old positions for
motion; geometry topology/material changes require a history reset. Lighting
changes must set `FrameInput::lighting_changed`. A lighting edit accelerates probe
coverage for eight frames, using 16-ray irradiance-only updates within the same
primary-ray budget. Geometry visibility/classification stays valid during the response.
Failed input validation preserves the last good extraction. Upload/build APIs
currently wait for completion; hosts should prepare replacements outside their
interactive frame loop and apply them at a frame boundary (D20).

## Passes and contracts

* Visibility: inline hardware triangle queries, stable external entity -> 24-bit
  scene ID, opaque/masked/stochastic-alpha candidate evaluation. RT hit, shadow,
  probe and reference paths use the same texture/normal/material evaluation.
* One automatic half-cell-padded probe volume (minimum 10x6x10, target 1.5 m spacing, at most 32,768 probes); 64 Fibonacci paths/update
  with a full sphere rotation, 6x6 octahedral irradiance and 8x8 distance moments.
  Seam-folded bilinear directional lookup removes quantized angular patches.
  Relocation follows the nearest back hit; relocated probes stay unclassified
  until sampled from their new origin. Geometric-normal and view bias, moment
  visibility, classification, irradiance-change detection and hysteresis bound
  leakage and temporal debt. Each budgeted primary path resolves up to five
  surfaces, plus explicit shadow rays; the 16384 budget counts primary paths.
* Primary visibility also evaluates full-rate direct lighting. Hardware-opaque
  queries traverse a separate opaque instance mask even in mixed extractions;
  masked/blended instances use candidate evaluation with closest-hit clipping.
  Shadow queries terminate on the first accepted blocker. Geometric ray origins
  prevent shading-normal offsets from starting rays inside tessellated surfaces.
* Bounded, importance-sampled GGX/diffuse final gathering resolves diffuse local occlusion; GGX visible
  normals resolve selective specular with bounded grazing-angle weights. The last
  event uses the probe field. Traced specular fades into directional probe lighting
  over 0.1 roughness below the cutoff, rather than an unoccluded sky fallback.
  Diffuse Fresnel uses a hemispherical approximation rather than view Fresnel.
* Transport uses compact checkerboard dispatches after initialization, plus a
  separate full-validity disocclusion repair dispatch. A skipped sample retains
  reprojected incident radiance; new geometry, illumination discontinuities and
  lighting resets receive new rays. Motion vectors remain unjittered UV deltas.
* Direct radiance has separate temporal history and never enters the incident
  radiance spatial blur. SVGF-style reconstruction validates ID/depth/normal/
  roughness/direct-light changes, validates bilinear history footprints, caps
  moving-surface history and accumulates incident moments. Rare bright bounce
  samples retain their energy; rough irradiance does not use luminance rejection. Three shared-memory 3x3 wavelet iterations use plane distance and
  roughness-aware normal weights. This preserves direct highlights, silhouettes
  and slanted surfaces. No NRD implementation code is copied.
* Hybrid radiance/normal/post intermediates use RGBA16F where possible; world
  positions, motion, hit records, moments and final HDR readback remain RGBA32F.
  Every reference intermediate and its accumulation are RGBA32F: repeatedly
  averaging thousands of samples into half precision loses reference energy.
* Engine TAAU keeps display-resolution history. FSR1 runs the official EASU/RCAS
  float implementation after transport temporal AA and the display transform.
  Motion is previous UV minus current UV, excludes jitter, and includes extracted
  vertex deformation. Reprojection explicitly adds previous minus current jitter.
* Exposure is `1 / (1.2 * 2^EV100)`, with stop compensation; auto exposure meters
  geometric-mean linear luminance and adapts using simulation delta. The metering
  grid is fixed 128x72 with 256 log-luminance bins over [-12,20] stops.
  Fractional low/high percentile trims and EV100 limits are typed settings;
  compensation is in stops, adaptation in inverse seconds. No wall clock or CPU
  readback drives adaptation. `capture_exposure` is an explicit diagnostic only.
* Energy-redistributing separable bloom; optional physical sky/aerial perspective and shadowed
  height fog through a frustum volume; depth-aware DOF and
  shutter motion blur behind the Cinematic quality gate.
* SDR selects Khronos PBR Neutral or the documented ACES fitted curve, then a trilinear
  16^3 grading LUT and piecewise sRGB transfer. ACES remains the compatibility default;
  it is not the full ACES color pipeline or HDR output.
* Reference mode uses the same hit/material/light functions, mixed GGX/diffuse
  importance sampling and Russian roulette, in batches of eight samples. It has
  no probes, denoising, variance clipping or temporal reconstruction. Accumulation
  is linear; exposure and the display transform happen afterwards.

`UpscalerInput` carries linear colour, positive view-space depth, current-to-prior
UV motion, render-pixel jitter, pre-exposure, reset and both sizes. Internal
transport is not pre-exposed, so `upscaler_input()` reports pre-exposure 1.
The packed motion image additionally stores expected prior radial depth and a
reactive-emission flag. `set_lut` takes linear RGB in G rows and B*16+R columns.
Scene material lowering is `ara_render::scene::Material`, including its texture
index contract; material AST integration should lower into this existing ABI.

## Remaining integration work

This is a verified rendering foundation, not completion of the full M3 brief.

1. **Native refit:** ara-gpu exposes one-time static AS only. Unchanged positions
   reuse BLAS, light/material-only edits skip TLAS, and changed positions rebuild
   only the affected BLAS before replacing TLAS. `AccelerationUpdate` reports
   same-topology refit candidates as explicit `refit_fallbacks`. Enabling actual
   refit needs an ara-gpu UPDATE/ALLOW_UPDATE API and lifetime tests, outside this
   task's permitted edits. Rigid transforms are currently lowered to world vertices.
2. **Raster/Mobile:** matching baked probes are required and validated by scene
   hash. These modes execute no AS creation or ray-query shader. Their standalone
   visibility path is a brute-force compute reference, suitable for small scenes,
   not a shipping raster replacement. Importing M1's raster G-buffer/shadow atlas
   is still needed for production non-RT performance. Mobile hardware is untested.
3. **NRD:** current SDK input contracts separate diffuse/specular signals and hit
   distances. This view separates direct radiance, but still filters diffuse and specular incident radiance together. The current
   [SDK license](https://github.com/NVIDIA-RTX/NRD/blob/master/LICENSE.txt) and
   [signal contracts](https://github.com/NVIDIA-RTX/NRD) were reviewed; NRD is not
   vendored and quality equivalence with NRD is not claimed.
4. **DLSS/Streamline:** the data seam exists; no DLSS binary is integrated. Public
   [Vulkan integration](https://github.com/NVIDIA-RTX/Streamline/blob/main/docs/ProgrammingGuide.md)
   requires loader/interposer or manual hooks plus device initialization, and the
   [DLSS guide](https://github.com/NVIDIA-RTX/Streamline/blob/main/docs/ProgrammingGuideDLSS.md)
   specifies application/project setup. The already-created ara-gpu device has no
   such initialization hook; changing it is outside ownership. FSR here means
   **FSR1**, not FSR2/3/4 or frame generation.
5. Probe scrolling/cascades, SSR, split-signal denoising, texture/asset streaming,
   async overlap, and full material-AST/editor product wiring remain to be built.
   This code uses the graph's existing serialized diagnostic execution; timestamps
   sum GPU pass durations, not CPU wall time or overlapped frame time.

The v1 heap has 64 storage-image slots: this view consumes 28. Resize temporarily
needs another 28; multiple editor views must account for that capacity. Textures
and the environment require a replacement view, while geometry/lights/material
parameters update in place. Call `destroy` before dropping a borrowed device.

See `../ara-pathtrace/README.md` for reproducible captures, errors and timings.

## Quality pass evidence

The apparent diagonal seam on the right diagnostic wall is the ceiling edge's
physical sun shadow: it also appears with DDGI disabled in the direct-only
reference (`../ara-pathtrace/captures/direct-shadow-reference.png`). Removing it
would disagree with the reference scene. Angular probe blocks and rim speckles
are reconstruction defects and are addressed by the changes above.

The original diagnostic views, resolution, RMSE definition, 96 settle frames and
4096 reference spp are retained. The reference now uses visible GGX normals with
its corresponding PDF; this changes sampling variance, not the BRDF. See the
before/after errors and per-pass GPU table in `../ara-pathtrace/README.md`.
Algorithms follow [Heitz's visible GGX distribution](https://jcgt.org/published/0007/04/01/)
and [visibility-aware irradiance fields](https://research.nvidia.com/publication/2019-05_dynamic-diffuse-global-illumination-ray-traced-irradiance-fields).
The probes and reconstruction are Ara implementations.

## Sponza energy/exposure round

PBR Neutral follows the [Khronos specification](https://github.com/KhronosGroup/ToneMapping/tree/main/PBR_Neutral).
It preserves midtone material colour while smoothly compressing/desaturating bright
highlights. This is an SDR display transform, not a scene-energy correction.
Goldens use fixed exposure; the content benchmark exercises histogram auto exposure
with -1.5 stops compensation. The unchanged physical Sponza lighting rig allows the
linear-radiance error to distinguish transport errors from display exposure.

Incident radiance is demodulated by a bounded material-colour factor before temporal
and wavelet filtering, then remodulated after filtering. BASE.w packs that UNORM8 RGB
factor as an exactly representable 24-bit integer; it no longer stores hit distance.
Primary visibility/direct lighting remain full rate. `set_transport_stride(4)` rotates
four compact incident-ray phases; uncovered invalid history is repaired at full rate.
A stationary camera integrates pixel-area coverage across a 256-phase Halton sequence;
moving views keep depth/normal/ID rejection. This avoids resampling each jittered
silhouette as though it were the same surface. Direct radiance averages independent
shadow samples separately; geometry changes retain illumination rejection.

`set_gather_events` bounds transport before the visibility-aware probe terminal.
Small-room default sampling remains two split lobes and half-rate transport. The
four-phase preset selects a single importance-sampled lobe with its selection PDF;
roughness-faded specular retains the same directional-probe blend. More events cost
more rays. Reset frames and views with extensive disocclusion cost more than the
steady-camera timing distributions. No async overlap is claimed.

Emissive triangles use area times luminance weighted CDF next-event estimation with
solid-angle PDFs and alpha-aware visibility. This is the sole emitter estimator
after the primary camera hit, so later BSDF hits omit emission to avoid double
counting. All lobes have finite roughness; no delta paths are dropped. Probe-origin
rays retain first-hit emission. Separate prewarmed emitter shader variants remove
this distribution and its register pressure from non-emissive scenes. Highly glossy
small-emitter paths would benefit from BSDF/light MIS. The classic mode remains
available; the optional reservoir mode below replaces its primary local-light
estimator and indirect gather.

The terrain fixture uses ara-landscape's actual sample tile and ara-terrain's
hole-aware heightfield mesh, with logarithmic distant terrain, branch geometry
and individually alpha-tested leaves with bounded two-sided transmission.
Production integration still needs tiled streaming/dirty-region BLAS updates,
material-layer/normal-map translation and foliage instance/wind previous positions.
The DLSS/Streamline seam and baked Raster/Mobile contracts are unchanged.

The terminal probe approximation accounts for both diffuse and specular energy;
conductors no longer become zero-energy terminals. It shares geometric visibility
and interpolation weights across diffuse and reflected directional irradiance
lookups, then applies the BRDF energy factors. It is a high-order filtered terminal,
not an exact directional radiance cache or a GGX convolution. XOR-folded frame digits
decorrelate the compact ray phases from Halton's horizontal subpixel strata.

## Lumen parity round 2

The corridor failure had two distinct causes. Temporal outlier clipping discarded
rare, important bounce paths. Remodulating averaged incident light with only the
current jittered albedo also broke pixel-area integration. The material factor now
accumulates alongside lighting (RGB10 packed into MOMENTS.z); variance comes from
its two moments. Luminance rejection ends by roughness 0.3, avoiding a second energy
loss as fresh history matures. Plane, normal, ID and roughness guides still protect
edges. Sharp reflections retain variance-based luminance weights.

`MaterialDetail::apply` tags extracted materials without changing the shared
80-byte material ABI. It carries coat, Charlie sheen, thin-sheet transmission,
authored ORM occlusion and a dielectric specular multiplier. Normal maps use UV
frames; the benchmark converts authored bump heights and derives tangent handedness.
MTL map_Ks affects specular intensity, never roughness. Optional layers use prewarmed
shader variants, so unused coat, sheen and atmosphere code does not burden Sponza.
Sheen base attenuation uses a baked 32x32 directional-albedo quadrature, reproducible
with `cargo run -p ara-pathtrace --example bake_sheen`. GGX samples below the
reflecting hemisphere are null events, including on transmitting leaves.

Material mip pyramids are packed into a single portable image because the current
GPU API exposes single-level allocations. Bilinear taps wrap inside each level;
interior taps use hardware filtering. Colour mips average linear light. Ray-cone
footprints select trilinear LOD, with log2(render/display) plus the typed view bias.
Normal variance broadens roughness to limit shimmer. Native stationary TAA reuses
already integrated pixel coverage without a redundant neighborhood reprojection.
Previous vertices are fetched only for primary motion vectors, not secondary rays.

`set_atmosphere(Some(Atmosphere { .. }))` enables Earth-scale Rayleigh/Mie/ozone
transport: 256x64 transmittance, 32x32 isotropic multiple-scattering transfer,
192x108 sky-view and 64x36x32 logarithmic frustum columns. The volume stores RGB
in-scattering and transmittance in separate atlas regions. It integrates height
fog, sun/local-light visibility and spot cones deterministically. Cumulative slice
ends interpolate from identity at the camera; an independent Beer-Lambert GPU test
checks this convention. The sky lights all transport, including probe rays and
reflections. The analytic sun disk has flux-normalized limb darkening. A lit planar
cloud layer contributes to sky radiance and shadows the sun/fog; volumetric clouds
are not implemented. Sun lux is top-of-atmosphere illuminance when this model is enabled.

The atmosphere layout follows [Hillaire 2020](https://sebh.github.io/publications/egsr2020.pdf)
with Ara quadrature/integration code. Layer equations follow the Khronos
[coat](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_clearcoat)
and [sheen](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_sheen)
models. The pinned OBJ/MTL has no authored coat or sheen parameters; the furnace
test exercises those layers independently. Production material-AST lowering,
landscape streaming and foliage wind still need the existing extraction seams.
Editor viewport output, FSR1/TAAU and the DLSS seam retain their contracts.

## Geometry / shadow integration contract

The public extraction boundary remains `ara_render::scene::Scene` through
`FidelityView::new` / `update_scene`; `GpuScene::instance_id(entity)` gives its
stable instance ID. `Material` is still 80 bytes and `Vertex` 48 bytes. Optional
`MaterialDetail` uses a version tag in reserved material flags. Geometry/VSM work
must preserve alpha cutoff, two-sided normals, previous deformed positions and
the extracted light units (lux, lumens, radiance). No root renderer changes are
required by this round.

The visibility-to-lighting layout below is unchanged. All images have render-size
dimensions, top-left UVs and right-handed Y-up world coordinates in metres.
These are internal graph targets, **not** a public writable-image import API;
an external visibility producer needs an additive graph adapter rather than
writing view-owned images behind the graph. That adapter must declare every
sampled/storage access and preserve this schema:

| Signal | Hybrid format | Channels and validity |
|---|---|---|
| WORLD, ping-pong | RGBA32F | world position xyz; exact stable instance ID + 1 in w; zero means sky |
| NORMAL, ping-pong | RGBA16F | camera-facing shading normal xyz; perceptual GGX roughness w |
| BASE | RGBA32F | triangle barycentrics xy, primitive index z, RGB8 demodulation factor packed as an exact 24-bit integer in w |
| MOTION | RGBA32F | previous UV minus current UV, excluding jitter, xy; previous radial camera distance z; reactive mask w |
| DEPTH | RGBA32F | positive current view depth x; zero means sky |
| DIRECT | RGBA16F | unexposed linear direct + primary emission rgb; surface-valid flag w |
| RAW | RGBA16F | demodulated indirect rgb; sample-valid flag w (quarter-rate phases leave holes) |

IDs/primitive indices must remain exactly representable as f32 integers. Barycentrics
refer to the same extracted triangles used by secondary hardware rays. A virtual
geometry producer therefore needs a correspondence to that ray scene. VSM visibility
may replace direct-light shadow evaluation; it must not multiply indirect lighting
by a second ambient-occlusion term. Normal/ORM sampling, layer energy and specular
occlusion remain shared with the reference material evaluator.

Camera matrices are unjittered for motion; the current inverse projection includes
jitter. History rejection adds the separately supplied jitter delta. `update_scene`
retains previous geometry for one frame; signal lighting edits with
`FrameInput::lighting_changed`, and cuts/reassignments with `reset`.
`upscaler_input` still exports scene-linear colour at pre-exposure 1, depth, motion,
jitter and both sizes. `output()` is the completed offscreen SDR image in CopySource
state; the editor must import/transition it before sampling, outlining or gizmos.

## Reservoir low-light mode

At a completed frame boundary, `set_reservoir_sampling(gpu, true)` selects Ara's
direct-light reservoirs and diffuse path reconnection. This RT-only mode is
explicit: classic DDGI remains the default and baked Raster/Mobile remains
available. Local lights are bounded at 4096; emissive triangles use the same
area-times-power distribution as the independent reference. Direct candidates
use independent emitter-position and reservoir-roulette random substreams.
An occluded rectangle test compares this estimator against numerical integration,
and a 1000-point-light test checks absolute BRDF/light energy after reuse.

DI reservoirs retain light ID, random seed, normalization, effective count and
visibility. Targets are reevaluated at the receiving surface. Temporal reuse
checks depth, normal, stable ID and roughness; spatial reuse reconnects only
compatible geometry. Visibility is reused only at an exactly matching receiver,
expires after eight frames and is discarded on lighting/geometry changes.
The design follows [ReSTIR DI](https://research.nvidia.com/publication/2020-07_spatiotemporal-reservoir-resampling-real-time-ray-tracing-dynamic-direct).

The GI reservoir stores the next vertex, normal and an eleven-event radiance
estimate. Area-measure reconnection includes the secondary cosine/distance-squared
Jacobian and retests visibility. Glossy secondary vertices and extreme Jacobians
limit reuse; rejected spatial proposals retain their effective sample counts.
This is an approximate, bounded path-reconnection estimator inspired by
[ReSTIR GI](https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing),
not an unbiased implementation of its full path-space MIS. Stationary rough
diffuse hits can terminate in previous screen-space radiance after ID, depth,
normal, maturity and lighting checks; off-screen/disoccluded hits trace paths.
This includes indirect energy and avoids a direct-only screen cache. Glossy
primary pixels retain full-rate GGX samples. Complex coat/sheen/
transmission materials retain full shared-BRDF path sampling instead of an
unsupported diffuse reconnection. The reference never uses reservoirs or caches.

Candidates occupy a rotating 2x2 grid; receivers reevaluate their targets at full
resolution. The compact trace pass additionally rotates the selected transport
stride. Thin geometry without a compatible candidate uses independent path
sampling. Private albedo/geometric-normal guides avoid repeating textured vertex
evaluation at each reservoir stage. Public G-buffer slots and `FILTER0=12` remain
unchanged; the feature/bloom integration test exercises the same HDR composition.

Diffuse and GGX specular signals have separate histories and moments. Two
variance-guided geometry-aware a-trous iterations use steps 1 and 3 in reservoir
mode; classic mode retains steps 1/2/4. Motion/light changes clamp
history to compatible current neighborhoods; static histories preserve rare
energy. Low-light histories use float32 because a long half-float EMA can lose
small corrections. Reprojection rejects disocclusions; lighting flags discard
reservoir reuse and use responsive weights. This is an Ara implementation, not
NRD/ReBLUR/ReLAX integration or a claim of equivalent quality. Reservoir resources
and extra guides are allocated on selection; a native view requires substantial
additional memory (six reservoirs alone use 288 bytes per render pixel).

## VSM sun source

`SunShadowSource::VirtualShadowMap` borrows a completed, render-size RGBA32F mask
from `ara_render_geo::shadows::VirtualShadows::output()`. RGB is linear sun
visibility; A>=0.5 means resident. Query `render_jitter(input)` before producing
the mask with the same camera, then publish it using `set_sun_shadow_source`.
The caller resolves the producer submission and retains the image through the
fidelity frame. The graph declares its Graphics/ShaderRead access. Invalid pages
fall back to scene ray queries; secondary receivers and local lights keep ray
visibility. Resize clears the borrowed mask. A GPU test checks black/white masks,
missing-page fallback, dimensions and jitter agreement. External virtual-geometry
surface import remains a separate adapter; this hook does not claim that wiring.
