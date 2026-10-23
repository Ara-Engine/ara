# Reference and fidelity verification

`Reference` progressively accumulates the shared transport kernel without GI
probes, denoising, history rejection or upscaling. Every sample is independent;
GGX/diffuse mixture PDFs and roulette weights preserve transport energy. Point
and directional visibility uses the same physical units and masked hit semantics
as hybrid rendering. Emissive triangles use area/power-weighted next-event estimation, with the same
alpha-aware visibility and finite-roughness material semantics as the hybrid.

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
Tests always write generated measurements to scratch space and read committed
goldens. CLI measurements and captures also default to ignored
`target/ara-pathtrace/` at the workspace root, or
`CARGO_TARGET_TMPDIR/ara-pathtrace/` when set. Checked-in `measurements/` and
`captures/` are updated only with `ARA_UPDATE_MEASUREMENTS=1` or the runner's
`-UpdateMeasurements` switch. Golden updates use the separate `-UpdateGoldens`
switch. `Check` clears both opt-ins; the round-3 wrapper publishes test receipts
only as an explicit step after successful checks with `-UpdateMeasurements`.
Keep one output policy for paired Sponza reference/quality runs so their generated
reference manifest matches the ignored HDR cache.

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
Lighting edits use responsive radiance weights while retaining valid geometric
history; disocclusion rejects history only where needed. The maximum central display residual must be <3.5% and reset residual <3%.
This is a bounded diagnostic, not a perceptual ghosting oracle.
The four-stop light-step test measures the probe field itself over 24 frames,
against a 48-frame target, and requires 90% response within 12 frames.

`measurements/` contains canonical JSON: per-case reference/convergence errors,
sequence errors, probe step response, device/driver identity,
and pass timing distributions. Generated probe bakes stay in ignored `.sn/`. `gpu-timings-before.json` records the original
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

## Prior 4a04cc3 quality-pass baseline

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
these are the historical measurements before the Sponza round below. The historical captures used a coarse probe volume and EV100 10; the new round
retains their data as a baseline, rather than using these numbers as current results.

Captures: `interior-before-quality.png`, `interior-1440p.png`, `interior-960p.png`,
`thin_wall-hybrid.png`, `glossy_interior-reference.png`, the three `*-sequence.png`
sheets, and `sponza-1440p.png` / `sponza-960p.png`, all under `captures/`.
The wall's diagonal edge is a correct ceiling/sun shadow; see direct-only reference
and explanation in the fidelity README. Native refit and production Raster/Mobile
remain blocked by the same backend integration boundaries documented there.
The editor offscreen target/material ABI and DLSS/Streamline seam are preserved.

## Sponza ground truth and robustness round

Reproduce using `run.ps1 -Mode Sponza-Reference`, then `-Mode Sponza-Quality`,
`-Mode Content`, `-Mode Robust-Quality` and `-Mode Robust-Timings`.
The local pinned Sponza asset must be fetched first. `Sponza-Reference` always
reaccumulates 4096 independent samples at 640x360 / 12 bounces, with a 2048-spp
convergence check. Its linear HDR cache is ignored; a matching scene fingerprint,
camera, resolution and sample contract are required before comparison. The
original baseline is immutable `sponza-baseline.json` (main 55bd504), with the prior
15.32/7.30 ms timing table retained in `sponza-timings-before-energy.json`.

`Sponza-Quality` integrates 192 jittered hybrid frames, enforces <=10% linear
relative RMSE, checks its fixed-EV100 5.5/PBR Neutral golden, and writes
`sponza-reference-side-by-side.png` (hybrid left, undenoised reference right).
Exposure/bloom/fog never enter the RMSE calculation. The reference comparison is
640x360; native/upscaled hero captures and GPU budgets are measured separately at
2560x1440 and 1707x960 ->2560x1440. No 1440p/4096-spp reference is claimed.
The automatic lattice has 6318 probes at about 1.4-1.5 m spacing, 5837 active after
settling. Probe rays resolve five scattering events before feeding the previous
visibility-aware field back into the terminal. No artificial GI gain, sky gain,
albedo boost or lamp was added to the Sponza rig.

Both the original hybrid and reference were underexposed at EV100 10. Histogram
metering is near EV100 4 before compensation; auto content captures use -1.5 stops
compensation. Fixed goldens use EV100 5.5 (Sponza), 12 (daylight), 5 (night), with
PBR Neutral. ACES-fit remains selectable. Photographic exposure is separate from
transport: passing global RMSE alone does not prove all shaded regions are correct.

`robust_scenes::outdoor` extracts an actual landscape sample tile, recentered in
metres, with heightfield vertex normals, a baked original layer-colour texture,
18 crossed alpha canopies and sky illumination. This is a terrain/visibility
fixture, not a production forest or realistic authored foliage asset. `night` has
five 12 cm emissive ceiling panels, zero environment/sun/analytic lights. Both have
480x270, 4096-spp /12-bounce same-view references and 256 hybrid settle frames.
Their fixed-exposure goldens are checked in `cargo test`; no historical before
measurement exists for these new fixtures. Robust timing JSON records per-pass
p50/p95 after 96 warmup frames. Sponza uses 240 samples after 96 warmup frames.
`Content` and `Robust-Timings` enforce <=10 /5 ms p95 and retain the measured
report even on failure. All runs use two Cargo jobs and API/synchronization validation. The shared GPU/CPU
have approximately ten agents, with no exclusive access or controlled clocks;
Sponza telemetry records nvidia-smi before/after each run. GPU-work sums are not
application frame latency, and extensive camera motion/reset frames cost more.

Native BLAS refit remains unavailable in ara-gpu's merged command interface; the
existing explicit rebuild fallback is retained. Raster/Mobile require matching
baked probes; a production landscape/foliage fallback still needs the material and
streaming integration described in the fidelity README. DLSS's SDK seam remains.

## Recorded final results

| Scene | Relative RMSE, before -> after | Native / upscaled p95, ms |
|---|---:|---:|
| Sponza | 27.12% -> 7.28% | 15.32 / 7.30 -> 8.99 / 3.93 |
| outdoor_day (new) | 3.94% | 5.03 / 2.90 |
| night_interior (new) | 4.91% | 6.81 / 2.86 |

Sponza 2048 ->4096 reference residual: 1.00%; daylight 0.11%; night 1.46%.
The legacy gates remain: glossy 6.99% ->6.47%, thin wall 9.64% ->9.04%, furnace
0.65% ->0.62%. The diagnostic frame is 6.44 /2.82 ms p95 (native /upscaled).

| Sponza pass p50, ms | Native before -> after | Upscaled before -> after |
|---|---:|---:|
| Bloom.Horizontal | 0.211 -> 0.208 | 0.097 -> 0.096 |
| Bloom.Vertical | 0.108 -> 0.108 | 0.052 -> 0.051 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.160 -> 0.141 | 0.157 -> 0.143 |
| Exposure.EV100.Meter.Adapt | 0.004 -> 0.025 | 0.005 -> 0.026 |
| Output.SDR | 0.046 -> 0.057 | 0.000 -> 0.000 |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.301 -> 0.310 | 0.094 -> 0.100 |
| RT.Disocclusion.Repair | 1.224 -> 0.000 | 0.693 -> 0.000 |
| RT.Reflections.SoftShadows.DDGI | 6.821 -> 2.733 | 3.100 -> 1.277 |
| SVGF.Atrous.1 | 0.496 -> 0.481 | 0.223 -> 0.219 |
| SVGF.Atrous.2 | 0.541 -> 0.546 | 0.258 -> 0.258 |
| SVGF.Atrous.4 | 0.591 -> 0.668 | 0.263 -> 0.296 |
| SVGF.Reproject.Reject.Outlier.Moments | 0.891 -> 0.868 | 0.387 -> 0.369 |
| Upscale.FSR1.EASU | 0.000 -> 0.000 | 0.289 -> 0.178 |
| Upscale.FSR1.RCAS | 0.000 -> 0.000 | 0.058 -> 0.063 |
| Upscale.TAAU.Resolve | 1.034 -> 0.580 | 0.000 -> 0.000 |
| Viewport.Ready | 0.000 -> 0.000 | 0.000 -> 0.000 |
| Visibility.Motion | 2.224 -> 1.629 | 1.008 -> 0.762 |

Each JSON also includes per-pass p95. Total p95 is measured from frame sums, not
the sum of individual pass percentiles. Static pixel-area accumulation removes
the repair pass from steady-view runs; the pan sequence exercises its moving-view
path, but is not a native-resolution performance gate. The timings do not prove
worst-case moving-camera/reset budgets.

Verified visually: all hybrid/reference/diff captures, native/upscaled hero views,
baked fallback, moving-light/time-of-day/disocclusion sheets and Sponza pan sheet.
**Historical round-1 verdict (40b2fa6):** this did not pass for an Unreal 5 Lumen
screenshot. Sponza then had no
authored normal/height maps or full PBR material conversion. At 640x360, its fixed
corridor crop [170,235]..[430,350] had 27.17% relative RMSE and 17.39% lower mean
luminance than the reference, despite 7.28% whole-image error. Bright architecture
dominates the global energy denominator. Lighting-reset sequences show visible
low-sample mottling, and the daylight alpha canopies are procedural stand-ins.
These limitations are visible in the committed captures; passing the numerical
gates is not visual parity.

The landscape fixture adds two existing local path dependencies. Per task file
ownership, the parent checkpoint supplied the generated Cargo.lock membership
edges. This follow-up changes no root files. Generated probe bakes are written to
ignored `.sn/baked-interior.json`; the fixture passes them to the fallback in memory.

The final detached verification run reaccumulated the three 4096-spp references,
passed strict display goldens, RMSE/temporal/budget gates, both crates' tests,
clippy with warnings denied, rustfmt checks and all 17 SPIR-V validations.
No capture PNG changed from checkpoint 1044f81; the result is deterministic.

## Lumen parity round 2

Commands: `-Mode Sponza-Reference`, `Sponza-Quality`, `Lighting-Sequence`,
`Robust-Quality`, `Robust-Timings`, then `Check`, with CARGO_BUILD_JOBS=2.
Sponza uses 32,768 independent samples at 640x360 / 12 bounces, including a
16,384-sample regional convergence check. The corridor is [170,235]..[430,350].
Both whole-image and regional radiance are measured before fixed EV100 5.5/PBR
Neutral. Cached references must match scene, camera and cooked shader cohort.
Material detail is enhanced in this round; each renderer uses the same materials
and lighting as its matched reference. This is not a 1440p reference claim.

`Lighting-Sequence` sweeps the sun over 16 frames, toggles a 12,000-lumen local light
on/off, and renders 192 frames after each event. Convergence uses filtered indirect
radiance (direct subtracted): eight consecutive ROI means must be within 5% of the
final 32-frame steady state. Frame-to-frame HDR ROI variance and per-pixel display
RMS (decoded linear sRGB) quantify flicker. Strip rows are sun/on/off; columns are
frames 0,3,7,31,160,168,176,184. These are simulation frames at 1/60 second.

The outdoor fixture extracts a real ARTH landscape tile, extends the actual
landscape height function through distant LODs, and adds branches, individual
serrated leaf cutouts, transmission and ground cover. It uses physical sky LUTs,
shadowed volumetric height fog and a lit planar cloud layer. Its 4096-spp reference
has independent surface transport but shares atmosphere LUTs/froxel composition;
HDR RMSE is before fog. This does not validate the medium against an independent
volumetric path tracer. The lower-half ROI separately records terrain/foliage error.
The night fixture remains emissive-only and uses the same bounded gather preset.

Furnace tests check coat, sheen and thin-sheet energy, including the GGX null-event
PDF contract. A separate GPU test checks atmosphere attenuation against the analytic
Beer-Lambert law. The sheen albedo LUT is an engine asset, not a capture/cache.
The OBJ, original copyright, HDR caches and intermediate cooks remain ignored.

The final timing table below uses the full steady RT frame, warm caches and 240
Sponza samples / 60 robust-scene samples. GPU pass sums are not CPU wall time or
async frame times. Reset, lighting-response and extensive-disocclusion peaks are
not covered by this steady-camera budget. Background load is recorded, including
CPU/free-RAM snapshots and GPU utilization/clocks; ten agents share the machine.
Failed prototype/contended timing runs are retained in the round-2 JSON report.

### Measured result

| Scene | Relative RMSE, before → after | Frame p95 ms native / upscaled, before → after |
|---|---:|---:|
| Sponza | 7.28% → 4.59%; corridor 27.17% → 8.96% | 8.99/3.93 → 9.16/4.31 |
| outdoor_day | 3.94% → 2.67% | 5.03/2.90 → 6.10/3.36 |
| night_interior | 4.91% → 4.90% | 6.81/2.86 → 7.54/3.53 |

Outdoor content changes from 2,084 proxy triangles to 49,496 landscape/leaf triangles.
Corridor mean luminance went from 17.39% too dim to 0.13% above its matched reference.
All errors compare each hybrid with its matched reference. The outdoor lower-half
ROI is 4.38%; night lower-half ROI is 15.83%, so its low global error hides a local gap.
The emissive night preset now uses two gather events and half-rate transport;
daylight retains one event and quarter-rate transport. This spends its room-scale
headroom on more small-emitter paths. The initial night 18.64% ROI result is retained in JSON.
The new atmosphere initialization passes cost extra on composition changes and are
not in these steady-frame distributions. No worst-case or exclusive-GPU budget is claimed.

### Per-pass GPU deltas

Median GPU milliseconds, old → new. Individual p95 values and retained failed
trials are in [lumen-round2.json](measurements/lumen-round2.json). Frame p95 above
comes from frame sums. Per-pass percentiles must not be added to estimate it.

**sponza**

| Pass | Native 1440p | 960p → 1440p |
|---|---:|---:|
| Bloom.Horizontal | 0.208 → 0.208 | 0.096 → 0.097 |
| Bloom.Vertical | 0.108 → 0.108 | 0.051 → 0.052 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.141 → 0.171 | 0.143 → 0.163 |
| Exposure.EV100.Meter.Adapt | 0.025 → 0.025 | 0.026 → 0.025 |
| Output.SDR | 0.057 → 0.054 | 0.000 → 0.000 |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.310 → 0.307 | 0.100 → 0.100 |
| RT.Reflections.SoftShadows.DDGI | 2.733 → 3.459 | 1.277 → 1.622 |
| SVGF.Atrous.1 | 0.481 → 0.508 | 0.219 → 0.234 |
| SVGF.Atrous.2 | 0.546 → 0.552 | 0.258 → 0.264 |
| SVGF.Atrous.4 | 0.668 → 0.635 | 0.296 → 0.287 |
| SVGF.Reproject.Reject.Moments | 0.868 → 0.836 | 0.369 → 0.369 |
| Upscale.FSR1.EASU | 0.000 → 0.000 | 0.178 → 0.222 |
| Upscale.FSR1.RCAS | 0.000 → 0.000 | 0.063 → 0.062 |
| Upscale.TAAU.Resolve | 0.580 → 0.300 | 0.000 → 0.000 |
| Viewport.Ready | 0.000 → 0.000 | 0.000 → 0.000 |
| Visibility.Motion | 1.629 → 1.594 | 0.762 → 0.735 |

**outdoor_day**

| Pass | Native 1440p | 960p → 1440p |
|---|---:|---:|
| Atmosphere.Froxels.AerialPerspective | 0.000 → 0.079 | 0.000 → 0.079 |
| Atmosphere.SkyView | 0.000 → 0.015 | 0.000 → 0.016 |
| Bloom.Horizontal | 0.206 → 0.206 | 0.095 → 0.097 |
| Bloom.Vertical | 0.107 → 0.107 | 0.050 → 0.051 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.238 → 0.117 | 0.243 → 0.117 |
| Exposure.EV100.Meter.Adapt | 0.024 → 0.023 | 0.024 → 0.026 |
| Output.SDR | 0.036 → 0.041 | 0.000 → 0.000 |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.214 → 0.370 | 0.058 → 0.172 |
| RT.Reflections.SoftShadows.DDGI | 0.628 → 1.257 | 0.304 → 0.622 |
| SVGF.Atrous.1 | 0.388 → 0.466 | 0.175 → 0.214 |
| SVGF.Atrous.2 | 0.417 → 0.491 | 0.201 → 0.232 |
| SVGF.Atrous.4 | 0.563 → 0.612 | 0.250 → 0.277 |
| SVGF.Reproject.Reject.Moments | 0.671 → 0.681 | 0.300 → 0.308 |
| Upscale.FSR1.EASU | 0.000 → 0.000 | 0.161 → 0.217 |
| Upscale.FSR1.RCAS | 0.000 → 0.000 | 0.050 → 0.056 |
| Upscale.TAAU.Resolve | 0.339 → 0.206 | 0.000 → 0.000 |
| Viewport.Ready | 0.000 → 0.000 | 0.000 → 0.000 |
| Visibility.Motion | 0.634 → 1.184 | 0.281 → 0.547 |

**night_interior**

| Pass | Native 1440p | 960p → 1440p |
|---|---:|---:|
| Bloom.Horizontal | 0.210 → 0.210 | 0.097 → 0.096 |
| Bloom.Vertical | 0.109 → 0.109 | 0.052 → 0.052 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.050 → 0.055 | 0.049 → 0.054 |
| Exposure.EV100.Meter.Adapt | 0.025 → 0.025 | 0.026 → 0.025 |
| Output.SDR | 0.042 → 0.041 | 0.000 → 0.000 |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.274 → 0.274 | 0.075 → 0.082 |
| RT.Reflections.SoftShadows.DDGI | 1.662 → 2.130 | 0.762 → 0.970 |
| SVGF.Atrous.1 | 0.454 → 0.490 | 0.205 → 0.222 |
| SVGF.Atrous.2 | 0.511 → 0.526 | 0.242 → 0.250 |
| SVGF.Atrous.4 | 0.643 → 0.627 | 0.283 → 0.280 |
| SVGF.Reproject.Reject.Moments | 0.915 → 0.902 | 0.400 → 0.421 |
| Upscale.FSR1.EASU | 0.000 → 0.000 | 0.160 → 0.213 |
| Upscale.FSR1.RCAS | 0.000 → 0.000 | 0.050 → 0.050 |
| Upscale.TAAU.Resolve | 0.494 → 0.263 | 0.000 → 0.000 |
| Viewport.Ready | 0.000 → 0.000 | 0.000 → 0.000 |
| Visibility.Motion | 0.780 → 1.091 | 0.350 → 0.523 |

### Response and visual inspection

| Event | Frames to 5% indirect steady state | Static ROI luminance-delta variance | Display pixel flicker RMS |
|---|---:|---:|---:|
| sun-sweep | 1 | 2.46e-07 | 0.523% |
| light-on | 1 | 9.9e-08 | 0.421% |
| light-off | 2 | 1.13e-07 | 0.491% |

Variance subtracts the mean delta; RMS retains drift. Flicker uses final frames
160–191 and has a 1% display-RMS gate in addition to visual strip review.
The steady-state target is a long-run hybrid, not a path-traced response target.

GPU/CPU load snapshots are in every current timing row. Contended/prototype Sponza
native p95 reached 13–19 ms; those failures are retained. The final workload uses
the single-loop directional-probe terminal, material mip fast path, primary-only
previous-position fetch and stationary TAA/temporal fetch fast paths. Frame batching
and a single elapsed timestamp span did not remove shared-load spikes and were rejected.

**Verified visually:** every committed final still, hybrid/reference/difference
image, native/upscaled hero and temporal strip; hero views inspected at full size.
The Sponza pair is [hybrid left / reference right](captures/sponza-reference-side-by-side.png);
the [sun/light strip](captures/sponza-lighting-sequence.png) shows no conspicuous flicker or trailing.

**sponza:** Not yet a finished UE5 Lumen cinematic: balanced bounce and detailed materials, but residual shadowed/contact detail softness; no general parity claim from one legacy asset view.

**outdoor_day:** No: real landscape geometry, atmosphere/fog and alpha leaves work, but sparse procedural vegetation, simple ground art and visibly planar clouds are below a UE5 outdoor screenshot.

**night_interior:** No: small-emitter mottling remains visible despite the denser gather; lower-half radiance RMSE is 15.83%; diagnostic geometry is not cinematic content.

The stable [geometry/VSM input schema](../ara-render-fidelity/README.md#geometry--shadow-integration-contract)
documents IDs, barycentrics, normals, linear lighting, previous radial depth and jitter.
External visibility import still needs an additive graph adapter; no private-image writes.
DLSS and editor offscreen output contracts are unchanged.

Verification: both crates passed tests, all-target clippy with warnings denied and
rustfmt checks; the denser night preset also passed the focused golden test. All 31
SPIR-V modules passed spirv-val for Vulkan 1.3. Validation and synchronization
validation reported no errors. Scratch/reference HDR caches remain ignored.

## Fidelity round 3

See [the round-3 report](FIDELITY_ROUND3.md) for low-light reservoirs, CC0 beauty content, independent references, temporal noise, shared-GPU median/p95/spikes, per-pass deltas and remaining UE5 quality gaps.
