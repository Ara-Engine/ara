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
`update_scene` changes extraction at a completed frame boundary, preserving GPU
IDs and unchanged BLASes. Position-only deformations retain old positions for
motion; geometry topology/material changes require a history reset. Lighting
changes must set `FrameInput::lighting_changed`. A lighting edit accelerates probe
updates for two complete budgeted sweeps, independently of the denoiser reset.
Failed input validation preserves the last good extraction. Upload/build APIs
currently wait for completion; hosts should prepare replacements outside their
interactive frame loop and apply them at a frame boundary (D20).

## Passes and contracts

* Visibility: inline hardware triangle queries, stable external entity -> 24-bit
  scene ID, opaque/masked/stochastic-alpha candidate evaluation. RT hit, shadow,
  probe and reference paths use the same texture/normal/material evaluation.
* One automatic 10x6x10 half-cell-padded probe volume; 64 Fibonacci paths/update
  with a full sphere rotation, 6x6 octahedral irradiance and 8x8 distance moments.
  Seam-folded bilinear directional lookup removes quantized angular patches.
  Relocation follows the nearest back hit; relocated probes stay unclassified
  until sampled from their new origin. Geometric-normal and view bias, moment
  visibility, classification, irradiance-change detection and hysteresis bound
  leakage and temporal debt. Each budgeted primary path resolves up to five
  surfaces, plus explicit shadow rays; the 16384 budget counts primary paths.
* Primary visibility also evaluates full-rate direct lighting. Hardware-opaque
  queries avoid material candidate loads when the entire extraction is opaque;
  masked/blended extractions retain candidate evaluation. Geometric ray origins
  prevent shading-normal offsets from starting rays inside tessellated surfaces.
* Two-surface cosine final gathering resolves diffuse local occlusion; GGX visible
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
  moving-surface history, rejects 12-sigma incident outliers and bootstraps fresh
  variance. Three shared-memory 3x3 wavelet iterations use plane distance and
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
  grid is fixed 32x18, rather than a percentile histogram.
* Energy-redistributing separable bloom; homogeneous single-scattering aerial
  perspective with Rayleigh/Mie sky phase approximations; depth-aware DOF and
  shutter motion blur behind the Cinematic quality gate.
* SDR uses the documented ACES fitted curve already used by M1, then a trilinear
  16^3 grading LUT and piecewise sRGB transfer. This preserves M1 display semantics;
  it is not the full ACES color pipeline, planetary atmosphere or HDR output.
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

The v1 heap has 64 storage-image slots: this view consumes 24. Resize temporarily
needs another 24; multiple editor views must account for that capacity. Textures
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
