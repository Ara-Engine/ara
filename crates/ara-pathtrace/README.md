# Reference and fidelity verification

`Reference` progressively accumulates the shared transport kernel without GI
probes, denoising, history rejection or upscaling. Every sample is independent;
GGX/diffuse mixture PDFs and roulette weights preserve transport energy. Point
and directional visibility uses the same physical units and masked hit semantics
as hybrid rendering. Emissive surfaces are found through BSDF sampling; explicit
area-light next-event estimation is not implemented yet.

Run from the repository root (all commands scope Cargo to two jobs):

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo run -p ara-shader-cook -- crates/ara-render-fidelity/shaders/pipelines.json crates/ara-render-fidelity/cooked
crates/ara-pathtrace/run.ps1 -Mode Check
crates/ara-pathtrace/run.ps1 -Mode Capture
crates/ara-pathtrace/run.ps1 -Mode Timings
crates/ara-pathtrace/run.ps1 -Mode Quality
crates/ara-pathtrace/fetch-sponza.ps1
crates/ara-pathtrace/run.ps1 -Mode Content
```

The runner isolates Windows implicit Vulkan layers exactly as ara-render does;
Khronos API and synchronization validation remain enabled. Captures are explicit
GPU readbacks; ordinary render frames do not copy image data to the CPU.
Goldens update only with process-local `ARA_UPDATE_FIDELITY_GOLDENS=1`.

`Check` runs crate tests, clippy with warnings denied, and rustfmt checks. The GPU
test compares seven committed 320x180 goldens at 1-SSIM <= 0.005, checks finite HDR
and white-furnace energy, verifies light edits retain all BLAS/TLAS identities,
checks a same-topology deformation rebuilds only one BLAS and retains its scene
ID, verifies an instance mask hides it, and resizes through odd FSR dimensions.

`Capture` uses 96 fixed-seed hybrid settle frames and a 4096-sample, 12-bounce
undenoised reference of the same camera. Linear RGB relative RMSE is RMSE divided
by RMS reference radiance, including visible background. Display 1-SSIM uses the
existing ara-render comparison. A separate 2048 -> 4096 linear error quantifies
finite-reference noise; the reference is not asserted to be fully converged.
The current residual is approximately 0.12% in the furnace and 1.35-1.7% indoors.
The quality gate additionally requires glossy <=8%, thin wall <=10%, furnace
<=2.5%, and reference 2048 ->4096 residual <3%; it runs in `cargo test`.

`captures/` contains same-view hybrid/reference/diff PNGs, the baked fallback,
12-capture moving-emitter/time-of-day/disocclusion sheets (all 48 simulation
frames rendered, every fourth frame captured), and 1440p native/FSR
readbacks. Temporal tests compare continuous accumulation against a separate
history-reset view with matching probe evolution in the central viewport region.
Global radiance history intentionally rejects lighting edits; disocclusion retains
history elsewhere. The maximum central display residual must be <3.5% and reset residual <3%.
This is a bounded diagnostic, not a perceptual ghosting oracle.
The four-stop light-step test measures the probe field itself over 24 frames,
against a 48-frame target, and requires 90% response within 12 frames.

`measurements/` contains canonical JSON: per-case reference/convergence errors,
sequence errors, probe step response, the baked probe field, device/driver identity,
and pass timing distributions. `gpu-timings-before.json` records the original
global-read denoiser; the current pass uses a cooperative shared-memory halo.

`Timings` uses 60 samples after 12 warmup frames at 2560x1440 and 1707x960 ->
2560x1440. The scene has seven instances, 4618 triangles, one point light and a
directional light, a 600-probe volume and a 16384 primary-probe ray budget. Post
includes auto exposure, bloom, fog/sky and grading; cinematic lens effects are
quality-gated off. GPU clocks are uncontrolled and validation is on. All queues
are serialized; sums are GPU work duration, not application frame latency.
Budget success on this diagnostic scene does not establish complex-content or
Unreal visual/performance parity. See `../ara-render-fidelity/README.md` for the
remaining native-refit, vendor SDK and production Raster/editor integration work.

## Quality pass measurements

All timestamps include API/sync validation on the RTX 4070 SUPER. Diagnostic
frame budgets are enforced against p95, with no async-overlap claim. Performance
for moving content and the first full-rate reset frame can differ from this
steady-view measurement. Full distributions and individual passes are in JSON.

| Linear relative RMSE vs 4096 spp | Before | After | Limit |
|---|---:|---:|---:|
| glossy_interior | 18.81% | 6.99% | 8% |
| thin_wall | 28.41% | 9.64% | 10% |
| furnace | 2.15% | 0.65% | 2.5% |

| GPU work, ms | Native 1440p p50 / p95 | 960p ->1440p p50 / p95 |
|---|---:|---:|
| Diagnostic full frame | 7.024 / 7.571 | 3.090 / 3.146 |
| Sponza full frame | 14.605 / 15.323 | 6.674 / 7.302 |

| Diagnostic GPU pass, p50 ms | Native | Upscaled |
|---|---:|---:|
| Bloom.Horizontal | 0.208 | 0.096 |
| Bloom.Vertical | 0.108 | 0.050 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.050 | 0.049 |
| Exposure.EV100.Meter.Adapt | 0.016 | 0.012 |
| Output.SDR | 0.036 | - |
| Post.Atmosphere.Lens.ACES.LUT | 0.255 | 0.075 |
| RT.Disocclusion.Repair | 0.565 | 0.283 |
| RT.Reflections.SoftShadows.DDGI | 1.628 | 0.745 |
| SVGF.Atrous.1 | 0.474 | 0.212 |
| SVGF.Atrous.2 | 0.515 | 0.243 |
| SVGF.Atrous.4 | 0.597 | 0.265 |
| SVGF.Reproject.Reject.Outlier.Moments | 0.842 | 0.367 |
| Upscale.FSR1.EASU | - | 0.281 |
| Upscale.FSR1.RCAS | - | 0.051 |
| Upscale.TAAU.Resolve | 0.951 | - |
| Viewport.Ready | 0.000 | 0.000 |
| Visibility.Motion | 0.758 | 0.352 |

Sponza is the actual 262,267-triangle model (25 material batches/materials,
26 textures), downloaded to gitignored `.sn/assets/sponza`. The pinned ZIP SHA-256
and attribution are in `fetch-sponza.ps1` and `sponza-timings.json`; source:
[Morgan McGuire's Computer Graphics Archive](https://casual-effects.com/data).
Original model: Frank Meinl / Crytek, 2010; archive corrections: Morgan McGuire,
2011. The original copyright file stays beside the local model. Assets are not
redistributed in this commit. The benchmark imports OBJ positions/vertex normals,
UVs, diffuse PNG textures and alpha masks, and approximates GGX roughness from
MTL shininess. It does not import height-bump maps or claim material parity with
an authored glTF PBR conversion. The large scene is **above both frame budgets**;
it has not been compared against a 4096-spp ground truth. One coarse automatic
probe volume also limits shaded-arcade quality. These limits remain visible in
its capture rather than being hidden by a simple-scene timing claim.

Captures: `interior-before-quality.png`, `interior-1440p.png`, `interior-960p.png`,
`thin_wall-hybrid.png`, `glossy_interior-reference.png`, the three `*-sequence.png`
sheets, and `sponza-1440p.png` / `sponza-960p.png`, all under `captures/`.
The wall's diagonal edge is a correct ceiling/sun shadow; see direct-only reference
and explanation in the fidelity README. Native refit and production Raster/Mobile
remain blocked by the same backend integration boundaries documented there.
The editor offscreen target/material ABI and DLSS/Streamline seam are preserved.
