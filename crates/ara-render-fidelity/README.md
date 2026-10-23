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
* One automatic 10x6x10 probe volume, 64 spherical Fibonacci rays/update, a strict
  primary-probe ray budget, 6x6 octahedral irradiance, 8x8 distance moments,
  relocation/classification, trilinear/normal/Chebyshev visibility weights,
  irradiance-change detection and hysteresis. Secondary shadow rays used to shade
  probe hits are additional to the declared 64-ray probe update budget.
* Directional and point/spot soft visibility; one GGX specular ray below a
  roughness cutoff, with rough environment/probe lighting outside that range.
* SVGF-style combined-radiance reprojection, stable-ID/depth/normal/roughness
  rejection, reactive emission, neighborhood variance clipping and three full
  5x5 B-spline a-trous iterations (1/2/4 pixel spacing). Shared-memory halo loads
  preserve the full kernel. No NRD implementation code is copied.
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
   distances. This view currently filters combined radiance. The current
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

The v1 heap has 64 storage-image slots: this view consumes 21. Resize temporarily
needs another 21; multiple editor views must account for that capacity. Textures
and the environment require a replacement view, while geometry/lights/material
parameters update in place. Call `destroy` before dropping a borrowed device.

See `../ara-pathtrace/README.md` for reproducible captures, errors and timings.
