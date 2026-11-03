# Published industry rendering costs: evidence for auditing render-v3

Research date: 2026-11-02. Scope: published engine/game measurements, not a new Ara profiling campaign. No engine changes or GPU runs. Read the project instructions, D29–D33, milestones/research synthesis, council-v3 architecture v9, and the current B1/B3 reports. D30/D31 concern project layout/editor appearance, not rendering-cost evidence. D33 is provisional and supersedes no defaults.

## Findings

**Yes, useful production breakdowns are published. They do not support a universal ordering such as “GI is the largest cost.”** DOOM publishes a whole-pipeline pass table; Unity publishes whole-frame before/after charts for Book of the Dead; Guerrilla publishes a whole-frame GPU timeline for its Killzone Shadow Fall announcement demo. Their disclosure is uneven: only the first has directly tabulated millisecond costs for all its broad pass groups. Unity's bar widths require approximate transcription; Guerrilla's timeline is useful structurally but is not a clean, additive timing table. [DOOM, p.3][doom] [Unity, p.71][unitypdf] [Killzone, pp.38–44][kzslides]

The strongest architectural lesson is to **avoid repeating work at a finer frequency than the image needs**, using culling, cached visibility, precomputed transport, decoupled shading, coarser representations, and overlap. These save different kinds of work; a faster ray primitive alone cannot replace all of them. DOOM's particle-light cache, Nanite's visibility/material separation, and Ghost of Tsushima's staggered probe relighting are concrete examples. [DOOM, pp.28–30][doom] [Nanite, pp.109–110][nanite] [Samurai Cinema, “Reflection Probe Relighting”][samurai]

Render-v3's combination of native resolution, no stochastic lighting, no screen-colour lighting history, exact combined source visibility, and demanding change deadlines removes several production escape routes. Published systems therefore supply mechanisms to study and workloads to test, **not proof of Ara's target cost or proof that the target is impossible**. The experiments below preserve the locked constraints.

## Reading the evidence

- `M`: author-reported measured/observed cost, not independently reproduced here. Approximate source numbers retain `~`. A talk's representative cost is not a p99 or worst-case certificate.
- `T`: target/budget, never substitute for a measurement.
- `G`: approximate reading of a published measurement graph. Its transcription uncertainty is additional to undisclosed measurement variance.
- `U`: undisclosed, unverified, or inseparable. It means missing data, not zero cost.
- Milliseconds throughout. Microseconds are divided by 1,000 explicitly where used. Source page numbers count PDF pages from one; slide titles identify HTML talks.
- Pass durations, GPU busy work, GPU critical-path span, CPU wall time, CPU worker time and presentation interval are different quantities. Do not sum asynchronous work, nested profiler rows, alternative settings, or different scenes into a frame time.
- These are within-publication examples, not a hardware-normalized engine ranking. Console generations, scene density, quality, material coverage and reconstruction differ substantially.

## 1. Full-frame / full-pipeline evidence

### A. DOOM (2016), id Tech 6

Sousa and Geffroy, SIGGRAPH 2016, **“The Devil Is in the Details: idTech 666,” p.3, “Anatomy of a Frame.”** The six rows cover the broad pipeline, rather than GI alone. The table itself does **not** identify a capture, percentile or exact GPU. The talk targets 1080p/60 Hz; p.40 explicitly labels a separate opaque-pass test PS4/1080p. Do not silently transfer that hardware label to p.3. Dynamic resolution is discussed on p.41. [Original course and authors][doomcourse] [Original PDF][doom]

| Stage | M, approximate ms | Included work / accounting limit |
|---|---:|---|
| Shadow caching | 3.0 | Shadow work; filter/generation split U |
| Pre-Z | 0.5 | Depth |
| Opaque forward | 6.5 | Clusters, texture composition, lighting, thin G-buffer outputs; material/direct/indirect split U |
| Deferred effects | 2.0 | Reflections, AO, fog, composite |
| Transparency | 1.5 | Particle lighting cache, VFX, glass |
| Post-processing | 2.5 | Explicitly async |

These are **not a measured 16 ms frame** merely because they add to 16. Post-processing overlaps the next frame's shadow/depth/opaque work (p.42). CPU submission, AS construction and isolated GI costs are not supplied by this table. Quality is the shipped pipeline, not a path-traced reference match. [DOOM pp.3,42][doom]

### B. Unity HDRP, Book of the Dead production-demo port

Unity's own 2021 article links Rob Thompson's **“Optimizing Performance for High-End Consoles”** talk and publishes the whole-frame chart; its 2023 optimization e-book reproduces it on p.71 and identifies the initial forest view as GPU-bound on **PS4 Pro, roughly 45 ms** on p.67. This is a demo-port optimization case, not an all-games HDRP benchmark. **Internal resolution, output resolution, exact Unity/HDRP revision, async configuration, sample count and variance are not disclosed alongside these charts.** [Official article and talk link][unity] [Unity-authored PDF, pp.67–71][unitypdf]

The following `G` values are visually read from the chart, rounded to roughly 0.5 ms. Allow approximately ±0.5 ms per broad segment; fine motion-vector/SSAO segments cannot be separated reliably. The changes include quality/performance decisions, not a certified equal-quality A/B. Do not interpret unchanged segment widths as exact invariant timings. [Original chart][unitychart]

| Broad stage, ms | Initial | After prepass changes | After shadow changes |
|---|---:|---:|---:|
| G-buffer + prepass | ~11 | ~6 | ~6 |
| Motion vectors + SSAO combined | ~1 | ~1 | ~1 |
| Shadows | ~14 | ~14 | ~8 |
| Lighting | ~5 | ~5 | ~4 |
| Atmospherics | ~6.5 | ~6.5 | ~4 |
| Post | ~7.5 | ~7.5 | ~8 |
| Published bar endpoint (read separately) | ~45 | ~40 | ~31 |

GI/reflections, transparency/particles, shadow filtering, scene updates and CPU submission are not independently itemized. “Lighting” must not be relabeled GI. The chart presents a stack but discloses no queue accounting, so it is not an async schedule certificate. This is trustworthy published whole-frame evidence with gaps; I did **not** find a fully specified, exact modern HDRP whole-frame table suitable for an apples-to-apples Ara comparison. [Unity p.71][unitypdf]

### C. Guerrilla, Killzone Shadow Fall announcement demo (2013)

The original developer post identifies the April 30, 2013 demo postmortem. Its PDF download timed out locally and exceeded the web reader's size limit; the **Guerrilla-uploaded** SlideShare copy supplied inspectable slide images. I inspected slides 38–44. This is the announcement demo, **not** a retail-build average and not modern Horizon/Decima performance. [Developer original][kz] [Author upload][kzslides]

The talk specifies PS4 hardware, 1080p rendering, a 30 Hz target, HDR deferred shading, low-resolution post-effect buffers, and job-generated command buffers. The city-view GPU timeline exposes geometry, lighting, reflections/post, effects, and final output work across the frame. Its highlighted regions and nested rows are **not** safe independent durations; no approximate millisecond dataset is fabricated from them. Exact quality preset, profiler overhead, async overlap and statistical coverage remain U. This is the third whole-frame source, but only a qualitative cost-distribution record here. [Slides 2,12,30,35–44][kzslides]

**Coverage result:** two usable numeric whole-pipeline publications (including three Unity frame variants), plus one whole-frame timeline. The additional sources below genuinely add subsystem evidence; they are not mislabeled additional full-frame breakdowns.

## 2. Chart-ready measured dataset

This CSV can be copied into a plot without interpreting blanks as zero. `context_id` joins to the mandatory provenance immediately below. `whole_frame_ms` intentionally stays empty for pass tables and component measurements. Plot separate panels by context; use independent bars for DOOM, not a stacked “frame time.” Unity's published stacks can be reproduced as approximate chart readings, with their original endpoints shown separately.

```csv
context_id,stage,cost_ms,evidence,whole_frame_ms,source_locator
doom2016,shadow_cache,3.0,M_approx,,PDF_p3
doom2016,depth,0.5,M_approx,,PDF_p3
doom2016,opaque_material_and_lighting,6.5,M_approx,,PDF_p3
doom2016,deferred_effects,2.0,M_approx,,PDF_p3
doom2016,transparency,1.5,M_approx,,PDF_p3
doom2016,async_post,2.5,M_approx,,PDF_p3
unity_initial,gbuffer_prepass,11,G,,PDF_p71
unity_initial,motion_vectors_ssao,1,G,,PDF_p71
unity_initial,shadows,14,G,,PDF_p71
unity_initial,lighting,5,G,,PDF_p71
unity_initial,atmospherics,6.5,G,,PDF_p71
unity_initial,post,7.5,G,,PDF_p71
unity_initial,frame_endpoint,,G,45,PDF_p71
unity_prepass,gbuffer_prepass,6,G,,PDF_p71
unity_prepass,motion_vectors_ssao,1,G,,PDF_p71
unity_prepass,shadows,14,G,,PDF_p71
unity_prepass,lighting,5,G,,PDF_p71
unity_prepass,atmospherics,6.5,G,,PDF_p71
unity_prepass,post,7.5,G,,PDF_p71
unity_prepass,frame_endpoint,,G,40,PDF_p71
unity_shadows,gbuffer_prepass,6,G,,PDF_p71
unity_shadows,motion_vectors_ssao,1,G,,PDF_p71
unity_shadows,shadows,8,G,,PDF_p71
unity_shadows,lighting,4,G,,PDF_p71
unity_shadows,atmospherics,4,G,,PDF_p71
unity_shadows,post,8,G,,PDF_p71
unity_shadows,frame_endpoint,,G,31,PDF_p71
```

Mandatory dataset provenance:

| Context | Hardware | Internal → output | Engine/date/workload | Async; omissions; quality |
|---|---|---|---|---|
| `doom2016`, every row | U on timing slide; console/GCN context, not certified PS4 attribution | 1080p target → 1080p target; actual timing-row internal size U | id Tech 6 / DOOM, SIGGRAPH July 2016, representative pipeline, capture U | Post overlaps next frame; no frame span, CPU, AS or separate GI; shipped feature grouping. [p.3][doom] |
| `unity_initial`, every row | PS4 Pro | U → U | HDRP / Book of the Dead forest port; chart published 2021, reproduced 2023; exact build U | Queue overlap U; no CPU/AS/separate GI; initial configuration. [pp.67,71][unitypdf] |
| `unity_prepass`, every row | Same | U → U | Same case, after prepass revision | Same gaps; equality of visual quality not established. [p.71][unitypdf] |
| `unity_shadows`, every row | Same | U → U | Same case, after shadow revision | Same gaps; final approximately 30 Hz-capable configuration, not a universal HDRP budget. [p.71][unitypdf] |

## 3. Component measurements and budgets — deliberately separate

### Epic: GI is neither the whole frame nor a constant cost

The version-pinned **UE 5.6 Lumen Performance Guide** gives **T=4 ms High / 8 ms Epic**, at **1080p internal → 4K TSR output**, for GI/reflections on opaque/translucent materials and volumetric fog, targeting console 60/30 Hz respectively. These are budgets, not measured full frames. It also documents async overlap, roughness-dependent reflection work, and TLAS rebuild costs scaling with active instances. Hardware, content and tracing configuration matter. [UE 5.6 guide][lumenperf]

The **SIGGRAPH 2022 Lumen** talk supplies actual component tables. All following rows are `M`, PS5, 1080p internal → 4K TSR, UE5 talk-era builds; source pp.192–195. High/Epic use different sampling rates. Scene images identify workloads; exact capture duration/percentile and async configuration are U. Excludes the rest of frame and does not separately price CPU/TLAS construction. [Original slides][lumentalk]

| Scene / configuration | Surface cache ms | Final gather ms | Reflections ms | Author's Lumen subtotal ms | Page |
|---|---:|---:|---:|---:|---:|
| Land of Nanite, SW global SDF, High | 0.47 | 2.07 | 0.23 | 2.77 | 193 |
| Same, Epic | 0.93 | 3.46 | 0.24 | 4.63 | 193 |
| Lyra, SWRT, High | 0.36 | 1.65 | 2.28 | 4.29 | 194 |
| Same, Epic | 0.66 | 3.30 | 4.74 | 8.70 | 194 |
| Matrix Awakens, HWRT + Far Field, High | 1.03 | 3.05 | 2.30 | 6.38 | 195 |
| Same, Epic | 1.17 | 5.52 | 4.62 | 11.31 | 195 |

These are author subtotals, **not GPU critical-path additions**. Smooth-material coverage changes the mix: reflections exceed final gather in Lyra, unlike Land of Nanite. The talk also describes separate static/dynamic SDF caches and modified-region updates (p.41), plus distance-dependent temporal amortization (p.169). [Lumen talk][lumentalk]

### Epic: geometry, direct lighting, shadows, reconstruction

| Evidence | Numeric result and context | What it does / does not establish |
|---|---|---|
| Nanite SIGGRAPH 2021, p.110 | M: ~2.5 ms visibility; ~2 ms material pass; average ~2496×1404 → 4K TAAU, original UE5-demo context. Timing slide does not explicitly name GPU. | GPU-driven visibility versus material evaluation; excludes lighting, shadows, post and scene-upload accounting. No whole-frame sum. Async HW/SW raster overlap is described on p.87. Exact sampled frame/statistic U. [Nanite][nanite] |
| Fortnite Chapter 4 TSR, official “Async Compute” section | M approximate: effective TSR 1.5 ms; critical-path finishing cost 1.1 ms; PS5/Xbox Series X, Battle Royale replay. Internal/output sizes and preset not fixed in this paragraph. | Different accounting of one subsystem, not two additive costs. CPU and other passes excluded; scene/time variation not tabulated. [TSR documentation][tsr] |

Nanite's original table visibly prints `108ms` for an instance-cull entry amid microsecond rows. I retain its explanatory headline values rather than silently repairing that apparent typo or constructing an exact subtotal. [p.110][nanite]

**MegaLights (UE 5.6 Beta)** trades exact enumeration for a fixed stochastic sample budget, shared BVH traversal, and denoising. More complex lighting can yield blur, noise or ghosting. VSM-backed lights additionally pay per-light preparation. The common BVH can be shared across lights, but this does not mean all visibility queries become free. It is evidence of a deliberate cost/quality tradeoff, not evidence that exact all-light native shading is equally cheap. [UE 5.6 MegaLights, “Technique Overview,” “Lighting Complexity”][megalights]

**VSM** allocates requested pages and retains valid pages. Moving lights/geometry can invalidate them; soft-shadow projection is a separate expense from making depths. Its use of shadow-map ray tracing approximates area-source visibility. These are useful mechanisms and explicit limitations, not proof that a continuously moving sun has steady-state static-cache cost. [Epic VSM, caching/performance/SMRT sections][vsm]

**TSR** reduces upstream pixel work using temporal reconstruction; later passes still operate at output resolution. A native-resolution requirement gives up this saving. It does not make all passes proportionally slower, since geometry, scene maintenance and caches have other scaling variables. The last sentence is an inference, not a measured scaling law. [TSR, “Hidden GPU Costs of Temporal Upscaling”][tsr]

### Sucker Punch: cheap updates have a constrained scope

Patry's **Real-Time Samurai Cinema**, SIGGRAPH 2021, reports these `M` timings on **base PS4**, Ghost of Tsushima (2020), talk-era implementation. Screen resolution does not define these probe-update workloads; output resolution is U for the timing tables. [Original HTML talk][samurai]

| Workload / table title | ms | Scope / overlap |
|---|---:|---|
| Sky luminance → SH, “Performance (Base PS4)” | 0.060 | Includes deringing |
| Full tetrahedral update, ~5,000 probes | 0.060 | Diffuse transfer update |
| Grid update, 73 tiles | 0.123 | Distinct stage, not whole GI |
| Reflection-probe relight | 0.347 | One probe |
| Mips / GGX convolution / BC6H compression | 0.037 / 0.250 / 0.060 | Same probe |
| Author's rounded probe total | ~0.700 | Async, one probe per frame, round-robin |

No frame-time inference from summing those rows. Probe data is captured offline; diffuse transport does not include local-light bounce, and movable/destructible objects are normally omitted from the bake. These are substantive limits behind the speed, not implementation bugs. The evidence supports amortization and streaming, not general dynamic topology repair with those costs. [Diffuse “Limitations,” specular performance/relighting sections][samurai]

### AMD Brixelizer GI: useful software baseline, not full frame

GDC March 2024, pp.54–56: default GI uses **0.5× internal scale**. `M` approximate GI figures: **RX 7900 XTX ~2 ms at 4K**, **RTX 4080 ~2.2 ms at 4K**, **RX 7600 XT ~1.7 ms at 1080p**. The slide presents defaults but does not attach an explicit preset/capture identifier to each timing. Do not advertise these as native-4K GI. CPU system: Threadripper PRO 3975WX, Windows 10; driver versions and hardware details are on pp.54,74. Async/critical-path overlap U; no complete frame or independently itemized SDF-update cost. [Original AMD presentation][brix]

Radiance injection depends on screen information; unseen changes can remain absent. Small details are lost. Memory accounting separately includes SDF atlas, internal buffers, cascades and scene-dependent scratch; the GI-only memory table is not total residency. This is a concrete counterexample to treating software GI as necessarily expensive, but its approximation and update policy differ materially from Ara. [pp.54–56,62][brix]

### Frostbite and RE Engine: other work matters

**Frostbite 2**, White/Barré-Brisebois, SIGGRAPH 2011, p.84: Xbox 360 tiled-lighting section reports `M` **4.0 ms point**, **7.8 ms point+specular**, **5.1 ms cone**, **5.3 ms cone+specular**, **5.8 ms line**, with **8 lights in every tile**; classification **1.35 ms including resolves**. These are alternatives, not additive stages; exact overlap with classification U. Internal/output resolution is not stated on that timing slide; full lighting model is supported, but shadow-cost inclusion is not certified. No GI/full-frame inference. [Original course PDF][frostbite]

The implementation shares tile depth/material classification and restricts light work to those tiles; postprocessing also shares passes and reduces sample frequency. These are dependency and data-layout decisions as well as arithmetic optimizations. [pp.17,74,84][frostbite]

**RE Engine**, Capcom's official RE:2023 talk, pp.8–13: static instances share BLAS; deforming objects need individual AS updates. Single-joint objects can become rigid instances. Refit-only destruction degrades traversal, motivating rebuild decisions; PC async compute is used for expensive dynamic BLAS work. No complete numerical frame breakdown was recovered from this talk. Its important contribution here is that **AS maintenance and traversal must be priced together**, not a claim about RE Engine's total speed. [Capcom author-published slides][reengine]

## 4. Audit against render-v3

The following are research judgments, not changes to D29/D32/D33. Native output need not logically prohibit every world-space cache or deterministic sharing mechanism, but it must not be used as a loophole to silently introduce reduced-resolution shading that the owner's brief prohibits. Screen-history reconstruction and world-space event caches are different; “no history” must not be misreported as a ban on D33's explicitly chosen caches.

| Assumption under audit | Evidence conflict or support | Classification / consequence |
|---|---|---|
| Industry GI targets justify a native-resolution complete Ara frame target | Lumen budgets/components omit much of the frame and use TSR; DOOM/Unity show substantial other costs. [Sections 1–3](#1-full-frame--full-pipeline-evidence) | **Unsupported aspiration** if used as a proof. Keep the target, replace the justification. |
| Dense exact per-source visibility can inherit MegaLights-style many-light scaling | MegaLights bounds samples and reconstructs; exact enumeration has a different work law. [MegaLights][megalights] | **Architectural scaling risk**, not automatically a bug. Need bound on unresolved pixel-light-source pairs. |
| A common visibility API implies common cheap work | Industry shares structures/caches, but different consumers have different resolution and update needs. [VSM][vsm] [RE Engine][reengine] | Interface unification is useful; backend cost sharing remains **unproven**. Count actual reused results. |
| Event-driven GI becomes cheap once steady | Sucker Punch's cheap relighting relies on baked transport and staggered updates; it does not prove dynamic graph repair. [Samurai Cinema][samurai] | **Workload-dependent amortization**, not a general cost certificate. Cold construction and event storms must remain visible. |
| GPU gather speed certifies whole transport | B3 reports unpacketized CPU discovery/forcing/receiver acquisition/publication and missing native timing. [B3 report](D:/AraBuild/render-v3/B3-report.md) | **Implementation gap** plus unmeasured end-to-end path. Moving code to GPU is necessary evidence work, not a guaranteed cure. |
| Correct patch residual means correct lighting | B3's current-triangle reference exposes active-receiver/media error despite small patch residual. [B3](D:/AraBuild/render-v3/B3-report.md) | **Representation/quality failure** independent of scheduling speed. Needs a different faithful receiver treatment within the gate. |
| Sparse links automatically satisfy memory limits | B3 records Polar host residency above the 4070S ceiling and Outpost's lower-layer footprint far beyond its ceiling. [B3](D:/AraBuild/render-v3/B3-report.md) | **Structural memory scaling** in current representation; not rescued by index-byte success alone. |
| Static sun-cache cost predicts moving-sun cost | VSM invalidation destroys reuse; architecture v9 itself flags low-angle demand and uncapped replay failures. [VSM][vsm] [v9](D:/AraBuild/render-research/council-v3/FINAL-architecture.md) | **Unproven amortization**. Must measure demand versus service capacity, including secondary callers. |

**B1 correction:** its low-LPP unshadowed **entire pipeline** is approximately **0.83 ms**, while lean shading is approximately **0.49 ms**. It is false to say that shading alone exceeds the **0.75 ms** local-light line. The full unshadowed path does exceed it in the reported experiment. Dense B1 is dominated by visibility, so reducing BRDF arithmetic alone will not address its principal failure. The report also identifies an open one-pixel energy defect: fix that correctness bug separately from the scaling problem. Stage p50s/p99s are not an additive frame p99. [B1, “Where the time goes”](D:/AraBuild/render-v3/gates/G1/B1-RESULT.md)

## 5. Prioritized experiments — proposed, not run

1. **Build an honest complete-frame budget ledger before another architecture claim.** Keep existing scenes/gates. For each workload, record GPU critical path and queue timelines separately from pass work; include geometry/materials, direct shading, depth generation, visibility filtering, GI, reflections, translucency/VFX/fog, AA/post, updates/uploads and CPU submission. Report cold, quiet, continuous motion and bursts independently. The deliverable is attribution and overlap, not a new faster headline. Industry sources above show why this accounting is prerequisite; this research did not run Ara.
2. **Resolve B1's pair-count scaling.** At fixed native resolution and authored ranges, sweep LPP, source size, blocker occupancy, moving fraction and transparency. Record candidates, certified clear/blocked pairs, unresolved pairs, source samples, traversal visits and shading invocations. Test deterministic conservative classification, result reuse and work compaction; verify the exact finite-range control. Keep per-source static/mover/translucent combination intact. Success means fewer expensive queries with unchanged error limits, not fewer shadowed lights by fiat.
3. **Separate B3 model error from incomplete execution.** First isolate active-body versus sleeping/current-triangle receivers at the same geometry. Then packetize and time discovery, forcing, repair, solve, upload and publication, including cold construction and oldest debt. Record total simultaneous residency, not only bytes/link. A fast kernel cannot resolve a wrong representation; a converged discrete operator cannot certify the continuous image. [B3](D:/AraBuild/render-v3/B3-report.md)
4. **Measure cache invalidation demand, not merely capped service.** Sweep sun motion/elevation, geometry motion, camera cuts, offscreen changes and reflection/fog demand. Track demanded pages/links, invalidation amplification, uncapped cost, service rate and age/error under caps. A growing queue fails even if dispatch time is flat. This tests the real benefit of D33's chosen caches without adopting temporal image history.
5. **Measure traversal together with maintenance.** Sweep instance overlap, long incoherent rays, repeated instances, deformation and destruction. Include TLAS/BLAS build/refit, stream-in, memory and hit-material evaluation. Keep software CWBVH as the gate backend. Optional hardware comparisons remain separately labeled diagnostics and cannot pass the software gate. Capcom's refit/rebuild distinction is the relevant lesson, not changing Ara to hardware RT. [RE Engine][reengine]
6. **Test legal spatial sharing at a fixed quality gate.** Compare deterministic material/light-list coherence, shared static visibility preparation, cache locality, and pass fusion. Study DOOM/Nanite patterns without assuming their reduced-frequency or temporal approximations satisfy Ara. Any proposed lower-resolution shading, stochastic sample selection, screen-colour history or relaxed deadline requires an explicit decision discussion; do not implement it under an optimization label.

Each experiment should have a predeclared failure interpretation: **bug** (wrong result or redundant accidental work), **scaling** (correct work grows with unresolved pairs/affected graph/instances), or **aspiration** (no complete measurement establishes the target). Preserve failed results. No published engine table alone decides whether Ara can meet its gates.

## Source and inspection ledger

- DOOM original SIGGRAPH PDF downloaded; p.3 timing table visually inspected. Nanite original PDF downloaded; p.110 performance table visually inspected, including its unit anomaly.
- Unity official article, original chart image, and Unity-authored 2023 PDF inspected; PDF p.71 rendered visually. The PDF is hosted in Unity's forum attachment storage, not an independent benchmark. The talk link was found through the official article; video playback was not used to invent additional numbers.
- Killzone original developer publication located; PDF fetch failed. Author-uploaded slides 38–44 inspected visually. No timeline pixel widths converted into unverified timings.
- Lumen original SIGGRAPH PDF downloaded; pp.193–195 rendered and visually checked against extracted tables; p.192 supplies resolution/settings context.
- Brixelizer original AMD PDF pp.54–55 rendered and inspected; pp.56,62,74 checked for limitations, previous-frame input and test system.
- Frostbite original SIGGRAPH PDF downloaded; p.84 table rendered visually. RE Engine official account's slide text checked, particularly pp.8–13; no numerical graph values extracted.
- Epic live default URLs currently resolve to a newer documentation version. Lumen and MegaLights claims are anchored to **5.6** URLs below; current VSM/TSR pages are explicitly documentation consulted on the research date, not measurements of all UE versions.
- No trustworthy fully specified frame table was recovered for modern Horizon/Decima or RE Engine. No secondary fan performance anecdotes or reconstructed game captures are substituted for developer measurements.

## Concise conclusion

Industry does publish cost breakdowns. The useful evidence is a **distribution of work and a list of deliberate savings**, not a single GI number. It supports studying visibility reuse, culling, representation size, update scheduling and complete-frame accounting before asserting render-v3's cost. It exposes gaps in the current justification while leaving the locked quality rules and promotion gates intact.

[doom]: https://advances.realtimerendering.com/s2016/Siggraph2016_idTech6.pdf
[doomcourse]: https://advances.realtimerendering.com/s2016/index.html
[unity]: https://unity.com/blog/games/expert-tips-on-optimizing-your-game-graphics-for-consoles
[unitypdf]: https://cdck-file-uploads-europe1.s3.dualstack.eu-west-1.amazonaws.com/unity/original/3X/1/2/12b84d9f9b078a386367f50be90a0e1bfcb04fea.pdf
[unitychart]: https://cdn.sanity.io/images/fuvbjjlp/production/de9b42df327ca4b06f8b37173bd871b474a1ae29-810x419.jpg
[kz]: https://www.guerrilla-games.com/read/killzone-shadow-fall-demo-postmortem
[kzslides]: https://www.slideshare.net/slideshow/killzone-shadow-fall-demo-postmortem/21167452
[nanite]: https://advances.realtimerendering.com/s2021/Karis_Nanite_SIGGRAPH_Advances_2021_final.pdf
[lumenperf]: https://dev.epicgames.com/documentation/en-us/unreal-engine/lumen-performance-guide-for-unreal-engine?application_version=5.6
[lumentalk]: https://advances.realtimerendering.com/s2022/SIGGRAPH2022-Advances-Lumen-Wright%20et%20al.pdf
[megalights]: https://dev.epicgames.com/documentation/en-us/unreal-engine/megalights-in-unreal-engine?application_version=5.6
[vsm]: https://dev.epicgames.com/documentation/en-us/unreal-engine/virtual-shadow-maps-in-unreal-engine
[tsr]: https://dev.epicgames.com/documentation/en-us/unreal-engine/temporal-super-resolution-in-unreal-engine
[samurai]: https://advances.realtimerendering.com/s2021/jpatry_advances2021/index.html
[brix]: https://gpuopen.com/download/GDC2024_GI_with_AMD_FidelityFX_Brixelizer.pdf
[frostbite]: https://advances.realtimerendering.com/s2011/White%2C%20BarreBrisebois-%20Rendering%20in%20BF3%20%28Siggraph%202011%20Advances%20in%20Real-Time%20Rendering%20Course%29.pdf
[reengine]: https://www.docswell.com/s/CAPCOM_RandD/ZRXJP2-RE2023
