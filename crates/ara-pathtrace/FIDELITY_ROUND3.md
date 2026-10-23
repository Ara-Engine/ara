# Fidelity round 3: low light, many lights and CC0 content

All timings below are **shared GPU** measurements on the RTX 4070 SUPER. The parent explicitly deferred a quiet-machine budget sweep. Median, p95, maxima, chronological frame times, per-pass distributions and two-second whole-device load samples are retained in [fidelity-round3.json](measurements/fidelity-round3.json). No exclusive reservation, forced clocks or async overlap was used.

## Independent references and error

Linear HDR relative RMSE; fixed exposure for goldens. References are unfiltered and share scene/material/light semantics. The round also corrects correlated emitter-position/roulette RNG, so before/after references differ in estimator correctness and sample count. Outdoor/beauty share the sky/fog display model; their surface HDR errors are measured before fog.

| Scene | Whole-image before → after | Surface ROI after | Reference spp | Half/full reference residual in ROI |
|---|---:|---:|---:|---:|
| Sponza | 4.59% → 4.59% | 8.96% | 32,768 | 2.80% |
| Night interior | 4.90% → 1.61% | 7.73% | 32,768 | 2.20% |
| 1000 lights | new → 10.21% | 17.36% | 65,536 | 3.06% |
| CC0 beauty | new → 3.17% | 4.65% | 4,096 | 1.09% |
| Landscape | 2.67% → 2.67% | 4.38% | 4,096 | 0.40% |

Night lower-half ROI improves **15.83% → 7.73%**, meeting the 8% target. The 1000-light fixture contains 500 point lights and 500 emissive panels; its ROI error is **17.36%**, with a 3.06% reference residual. This is a stress measurement, not a claim of comparable image quality.

## GPU time

Milliseconds are median / p95 / maximum; 1440p native or 1707×960 → 2560×1440. These are complete graph pass sums after warmup, with auto-exposure enabled.

| Scene | Native median / p95 / max | Upscaled median / p95 / max | Previous native / upscaled p95 |
|---|---:|---:|---:|
| Sponza | 8.703 / 9.908 / 10.518 | 4.128 / 4.807 / 5.212 | 9.165 / 4.305 |
| Night interior | 10.313 / 11.491 / 11.665 | 4.714 / 4.819 / 5.019 | 7.538 / 3.534 |
| 1000 lights | 12.040 / 13.413 / 14.755 | 5.394 / 6.260 / 7.065 | new |
| CC0 beauty | 6.905 / 7.531 / 8.506 | 3.410 / 3.612 / 3.919 | new |
| Landscape | 5.497 / 5.736 / 5.899 | 2.800 / 2.999 / 3.506 | 6.100 / 3.360 |

The night upscaled p95 increment is **+1.286 ms** over the accepted 3.533664 ms baseline, so the +1 ms timing gate **does not pass in this shared run**. Sponza meets 10/5 ms in this run. Whole-device telemetry spans 0–97% GPU utilization and 2944–5388 MiB VRAM. These samples include our own work; spikes cannot be attributed solely to other agents.

### Per-pass deltas

Median ms before → after. A dash means that pass did not exist. Per-pass p95 and raw frame spikes are in the JSON; summing pass percentiles is not a frame percentile. Reservoir filter names identify base steps; pixel spacing scales above 270p.

**Sponza**

| Pass | Native before → after | Upscaled before → after |
|---|---:|---:|
| Bloom.Horizontal | 0.208 → 0.209 | 0.097 → 0.097 |
| Bloom.Vertical | 0.108 → 0.109 | 0.052 → 0.052 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.171 → 0.180 | 0.163 → 0.184 |
| Exposure.EV100.Meter.Adapt | 0.025 → 0.026 | 0.025 → 0.026 |
| Output.SDR | 0.054 → 0.056 | — → — |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.307 → 0.319 | 0.100 → 0.099 |
| RT.Reflections.SoftShadows.DDGI | 3.459 → 3.310 | 1.622 → 1.545 |
| SVGF.Atrous.1 | 0.508 → 0.507 | 0.234 → 0.230 |
| SVGF.Atrous.2 | 0.552 → 0.560 | 0.264 → 0.267 |
| SVGF.Atrous.4 | 0.635 → 0.562 | 0.287 → 0.248 |
| SVGF.Reproject.Reject.Moments | 0.836 → 0.886 | 0.369 → 0.376 |
| Upscale.FSR1.EASU | — → — | 0.222 → 0.176 |
| Upscale.FSR1.RCAS | — → — | 0.062 → 0.065 |
| Upscale.TAAU.Resolve | 0.300 → 0.327 | — → — |
| Viewport.Ready | 0.000 → 0.000 | 0.000 → 0.000 |
| Visibility.Motion | 1.594 → 1.599 | 0.735 → 0.737 |

**Night interior**

| Pass | Native before → after | Upscaled before → after |
|---|---:|---:|
| Bloom.Horizontal | 0.210 → 0.209 | 0.096 → 0.096 |
| Bloom.Vertical | 0.109 → 0.109 | 0.052 → 0.052 |
| DDGI.Trace.Relocate.Classify.Integrate | 0.055 → 0.062 | 0.054 → 0.061 |
| Exposure.EV100.Meter.Adapt | 0.025 → 0.026 | 0.025 → 0.026 |
| Output.SDR | 0.041 → 0.044 | — → — |
| Post.Atmosphere.Lens.ToneMap.LUT | 0.274 → 0.279 | 0.082 → 0.081 |
| RT.Reflections.SoftShadows.DDGI | 2.130 → — | 0.970 → — |
| ReSTIR.Candidates.Temporal | — → 0.737 | — → 0.355 |
| ReSTIR.GI.CompactTrace | — → 1.584 | — → 0.736 |
| ReSTIR.Spatial.Visibility.Shade | — → 1.377 | — → 0.653 |
| ReSTIR.Specular.Dense | — → 0.310 | — → 0.161 |
| SVGF.Atrous.1 | 0.490 → 0.958 | 0.222 → 0.345 |
| SVGF.Atrous.2 | 0.526 → — | 0.250 → — |
| SVGF.Atrous.3 | — → 0.916 | — → 0.481 |
| SVGF.Atrous.4 | 0.627 → — | 0.280 → — |
| SVGF.Atrous.8 | — → 1.058 | — → 0.426 |
| SVGF.Reproject.Reject.Moments | 0.902 → 1.545 | 0.421 → 0.695 |
| Upscale.FSR1.EASU | — → — | 0.213 → 0.162 |
| Upscale.FSR1.RCAS | — → — | 0.050 → 0.052 |
| Upscale.TAAU.Resolve | 0.263 → 0.271 | — → — |
| Viewport.Ready | 0.000 → 0.000 | 0.000 → 0.000 |
| Visibility.Motion | 1.091 → 0.755 | 0.523 → 0.331 |

## Temporal noise and visual checks

| Static sequence | 16×16 block luminance delta RMS before → after | Pixel luminance delta RMS before → after |
|---|---:|---:|
| Night | 0.50% → 0.30% | 4.02% → 1.12% |
| 1000 lights | 1.06% → 0.47% | 2.85% → 1.26% |

Noise ablations compare current independent RIS/classic GI against reservoirs, not a recompiled 8a40d3d. They use 256 warmup + 64 fixed-camera frames at 480×270. The equal-area resolution test compares 270p/540p against an independent 8192-spp reference: **8.59% / 6.52%**, confirming that higher resolution no longer exposes stronger low-frequency noise. The main night score uses the separate 32768-spp reference.

Native inspection caught the original fixed-pixel filter failure despite the passing 270p score. The corrected filter scales its angular footprint and keeps plane rejection in world units. Native/upscaled walls are much smoother, but structured wall mottling and edge seams remain visible in the 270p night strip. The blotch-free strip target is not met. Direct/specular fine grain also remains; this is not full ReBLUR/ReLAX or unbiased path-space MIS.

My screenshot judgement against a polished equivalent UE5 Lumen view:

- **Sponza — no.** Below a polished UE5 cinematic: balanced bounce and detailed fabric/stone, with softer contact and shadowed detail.
- **Night interior — no.** Below UE5 polish: native/upscaled walls are much smoother, but the 270p strip still has structured wall mottling and edge seams; fine specular grain and softened contacts remain.
- **1000 lights — no.** Below UE5 polish: the stress view still has residual noise and structured mottling; the lower-half reference error remains high.
- **CC0 beauty — no.** Below a finished UE5 archviz shot: authored PBR furniture reads well, but the room/window treatment is sparse and silhouettes/contact detail are softer.
- **Landscape — no.** Below UE5 outdoor quality: sparse procedural vegetation, simple terrain art and visibly planar clouds.

## Captures and integration

| Scene | Native | Upscaled | Hybrid / reference |
|---|---|---|---|
| Sponza | [sponza-1440p.png](captures/sponza-1440p.png) | [sponza-960p.png](captures/sponza-960p.png) | [sponza-reference-side-by-side.png](captures/sponza-reference-side-by-side.png) |
| Night interior | [night_interior-1440p.png](captures/night_interior-1440p.png) | [night_interior-960p.png](captures/night_interior-960p.png) | [night_interior-side-by-side.png](captures/night_interior-side-by-side.png) |
| 1000 lights | [many_lights-1440p.png](captures/many_lights-1440p.png) | [many_lights-960p.png](captures/many_lights-960p.png) | [many_lights-side-by-side.png](captures/many_lights-side-by-side.png) |
| CC0 beauty | [beauty_cc0-1440p.png](captures/beauty_cc0-1440p.png) | [beauty_cc0-960p.png](captures/beauty_cc0-960p.png) | [beauty_cc0-side-by-side.png](captures/beauty_cc0-side-by-side.png) |
| Landscape | [outdoor_day-1440p.png](captures/outdoor_day-1440p.png) | [outdoor_day-960p.png](captures/outdoor_day-960p.png) | [outdoor_day-side-by-side.png](captures/outdoor_day-side-by-side.png) |

[Night strip](captures/night-noise-strip.png), [1000-light strip](captures/many-lights-noise-strip.png), [sun/light changes](captures/sponza-lighting-sequence.png), [camera pan](captures/sponza-pan-sequence.png). Native/upscaled stills, reference pairs and temporal contact sheets were inspected.

The beauty golden arranges authored CC0 Poly Haven furniture with original PBR maps. Inputs are pinned in assets/beauty-manifest.json and fetched with fetch-beauty.ps1; large files remain ignored. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). The strict fixture glTF reader is not the engine material/import pipeline.

G-buffer semantics, 80-byte Material, 48-byte Vertex, first 28 targets, FILTER0=12 and seven uploaded read/write rows remain stable. The VSM sun-mask selector is tested with resident/missing pages and jitter; virtual-geometry surface import still needs its graph adapter. Editor HDR composition/bloom tests pass. Baked fallback and the DLSS seam remain available; no new production Raster/Mobile performance claim is made.

Reproduce with `./verify-round3.ps1 -SharedGpu` after fetching the pinned assets. `-TimingsOnly` reruns four timing modes without changing quality receipts; omit `-SharedGpu` for strict budget gates. The public wrapper was syntax-checked; equivalent individual stages produced these receipts.

Validation: both crates pass cargo test --all-targets, clippy --all-targets with warnings denied, fmt checks, all 48 Vulkan 1.3 SPIR-V validations, hardware validation and synchronization validation. The separately recorded shared-GPU night timing gate fails as described above.
