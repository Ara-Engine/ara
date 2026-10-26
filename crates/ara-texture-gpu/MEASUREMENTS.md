# GPU texture compression qualification

Measured 2026-10-25 on the shared Windows 11 / Ryzen 7 7800X3D / RTX 4070 SUPER
machine. Cargo jobs were capped at two. Timing runs used `ara-dev gpu`; the
encoder creates a headless Vulkan device. No window or pre-imported template
content cache is involved. Cold project import results are recorded separately
in `../ara-importers/MEASUREMENTS.md`.

## Determinism and fallback

The CPU packs quantized block inputs once. Codec error calculations, reductions,
candidate order and final tie breaking use integer arithmetic. BC7 HQ retains
285 mode/partition/rotation/index combinations; Balanced retains 85. BC6H HQ
retains all 14 modes and 32 partitions. Scalar, optional packed-dot and split
dispatch implementations have byte-equality tests against frozen references.
Cross-vendor execution has **not** been measured; the available device is NVIDIA.

The backend identity includes the immutable cooked shader cohort. Preset and
backend participate in action and intermediate texture cache keys. Device names,
driver timing and thread budgets do not. Missing Vulkan, unsupported features or
a failed encode select ISPC under its CPU key, never publish CPU bytes under a
GPU key. Worker negotiation tests verify this boundary and binary-framed stdout.
Pipeline/device diagnostics go to stderr.

Normal-map imports retain ISPC BC5 for both presets. All 135 sample normal
contents, including every mip, are byte-identical to the qualified CPU outputs.
The raw GPU BC5 API exists, but its six-entry endpoint mode produced a localized
29.17-degree increase in compression angular error on the helmet normal map and
was slower than two-thread ISPC. Higher aggregate PSNR does not qualify that
regression. BC4/BC5 GPU reference tests remain useful without making it the
production normal-map path.

## Kernel measurements

BC7: medians for 65,536 representative blocks, including all 18 dispatches and
barriers. Both columns retain the same encoded bytes; specialization and packed
integer dots change scheduling/arithmetic, not the candidate search.

| Implementation | Balanced GPU ms | HQ GPU ms |
|---|---:|---:|
| Initial split scalar | 16.49 | 42.80 |
| Mixed packed-dot, protocol 2 | 14.52 | 37.47 |
| Protocol 2 host wall | 14.96 | 38.00 |

An earlier monolithic-to-split comparison measured 160.19 -> 16.38 ms Balanced
and 262.21 -> 43.16 ms HQ. A constructed nonuniform lossless mode-6 workload
improves to 1.30 / 2.11 ms with exact zero-winner pruning; that workload is not
representative sample throughput. BC7 scratch is 1,552 bytes per block, bounded
to 65,535 blocks per submission.

BC6H: one warm full-chain run of the 1024x512 sample sky, 11 mips / 43,693 blocks,
four CPU threads. Pipeline initialization is outside these timings. Encoded bytes
match the prior GPU implementation.

| Preset | ISPC ms | Monolithic GPU host ms | Split GPU host ms |
|---|---:|---:|---:|
| Fast | 25.926 | 33.827 | 13.797 |
| Balanced | 45.204 | 337.207 | 24.546 |
| HQ | 338.336 | 2314.251 | 242.490 |

This is 1.84x / 1.40x faster than ISPC for Balanced / HQ, not 20-50x. The complete
20-pipeline harness initialized in 9.214 s; production loads only the selected
preset's kernels. BC6H scratch is 344 bytes per block. Other HDR content has not
been broadly qualified.

Cached coherent readback allocation reduced the median CPU copy of a 4 MiB
buffer from 18.473 to 0.531 ms, with equal bytes and clean Vulkan validation.
That is a 34.8x improvement in the readback operation, not total import time.
GPU timestamp spans can overlap; do not add upload/encode/readback spans to
construct end-to-end wall time. Use measured host elapsed time.
Different worker devices can also overlap timestamp spans on the physical GPU;
their summed encode spans are not serial compute time or a hardware floor.

## Full-texture colour quality

Independent `bcdec_rs 0.2.1` decoding covered 142 distinct albedo contents and
135 normals from Outpost and Showcase. The frozen integrated host used scalar
split BC7; subsequent exact optimizations preserve its output bytes. Texture
decode, mip generation and all mip compression used a private two-thread Rayon
pool. These sequential corpus timings exclude source file I/O and QA decoding;
they are not daemon/project import times or warm-repeat medians.

| Preset | GPU importer s | ISPC importer s | GPU compression s | ISPC compression s | Weighted GPU RGB PSNR | Weighted ISPC RGB PSNR |
|---|---:|---:|---:|---:|---:|---:|
| Balanced | 40.995 | 113.328 | 33.737 | 105.748 | 49.289 dB | 49.348 dB |
| HQ | 45.881 | 369.412 | 38.548 | 362.204 | 49.526 dB | 49.465 dB |

Quality comparisons include source texels outside mesh UV coverage. RGB PSNR
uses sRGB code values; linear-light and alpha metrics are also retained. The
worst whole-texture RGB PSNR losses against ISPC HQ were human_suit: 4.232 dB
Balanced / 4.150 dB HQ, at GPU PSNR 57.327 / 57.409 dB and maximum RGB error
2/255. WaterBottle had the largest linear-light loss, 8.839 / 8.803 dB; its RGB
losses were 3.49 / 3.37 dB. Those relative losses must not be represented by the
much smaller corpus-weighted average.

Denim is the lowest absolute-quality case: GPU Balanced 38.910 dB versus ISPC
Balanced 39.205 and ISPC HQ 39.228; GPU HQ is 39.387 dB. Enlarged weave crops
show more local contrast/detail loss in GPU Balanced than HQ. Whole textures
remain close in the inspected sheets. Human-suit/WaterBottle flat regions show
small colour shifts. Shrub leaf detail remains close; alpha cutoff differences
are not zero: source-alpha>=128 mismatches are 190 ISPC HQ, 241 GPU Balanced,
240 GPU HQ of 1,048,576 texels. The source-visible worst shrub block is retained
in the report rather than dismissed as transparent RGB.

Inspected source/CPU/GPU sheets include human_suit, WaterBottle, denim, shrub,
and CPU normal maps with angular-error and neutral-lit plane/sphere views.
These are decoded texture checks, not rendered scene acceptance. The HDR sky
checks found lower linear RMSE for GPU HQ (1.411 versus ISPC 4.825), but slightly
worse log RMSE (0.002517 versus 0.002462), with local/coarse-mip regressions.

## Evidence and focused checks

Raw commands, source/library hashes, per-texture quality/timing CSVs and labelled
sheets live under the dedicated validation target's `gpu-codec-bc7`,
`gpu-codec-bc45/gpu-integrated-quality` and `gpu-codec-bc6h` directories. Selected
reports and sheets are retained in this worktree's ignored
`target/ara-dev/import-speed/retained-gpu-quality/` before target cleanup.
The first corpus wrapper lost its numeric exit status; all 277 cases reached
ENCODE_COMPLETE without engine errors. Its analysis/render wrapper completed
with exit 0 after fixing exit-status capture; the encode was not repeated just
for that bookkeeping issue.

Focused probes exercise every format/preset, independent decoder/reference
comparisons, scalar/dot equality, over-dispatch guards, malformed input,
cancellation, transfer reuse and a multi-chunk tail. The production-worker GPU
test verifies negotiation, exact result bytes and protocol stdout. Both are
ignored in ordinary CPU tests and run explicitly through the machine GPU lock.
No GPU-vendor portability or 20-50x end-to-end claim follows from these results.

## Leased device reuse: 2026-10-26

Two lazy headless devices now retain pipelines and transfer allocations across
disposable native import workers. The authenticated helper belongs to a private
engine profile, immutable shader cohort and worker executable/library fingerprint.
Host/job leases and active clients keep it alive; closing the last connection
starts a two-second shutdown grace. Helpers run content-addressed executable and
library copies under a writable cache, so they do not lock Cargo build outputs.
`ARA_IMPORT_RUNTIME_DIR` can place those copies in an explicit owned directory.
`ARA_TEXTURE_ENCODER_DEVICE=local` retains the former per-job route for controls.
Routing, authentication and executable paths do not change import keys or bytes.

The final matrix used merged source `e8ca4b61`, four admitted import workers,
Balanced, the same frozen art/player and the real `ara hub create` then daemon
open/import path. Every case had fresh project/action/object/intermediate and Ara
Vulkan caches. OS file pages and the vendor driver disk cache were not flushed.
This development build has opt-level 1, optimized dependencies and synchronization
validation enabled. Other agents were active; no owned build or CPU burner ran
during the matrix. All cases ran under one `ara-dev gpu` lock, with daemon/helper
shutdown confirmed between cases. Times end at all assets published, not at an
interactive installed editor's first usable frame.

Arms ran A/F/L/P/P/L/F/A; each arm includes both samples:

| Arm | Showcase runs, s | Outpost runs, s |
|---|---:|---:|
| A: frozen original `92e0b1a3`, old sidecars, local devices | 12.576 / 35.963* | 36.077 / 33.826 |
| F: frozen fair-admission worker `8bbc0db8`, old sidecars, local | 13.037 / 14.775 | 36.018 / 34.135 |
| L: candidate, current sidecars, local | 13.699 / 12.820 | 33.018 / 53.681* |
| P: same candidate and sidecars, pooled | 11.663 / 11.218 | 26.425 / 24.615 |

An asterisk retains an I/O outlier: a `cache.install` span exceeded the declared
five-second threshold. A2 Showcase reached 13.004 s; L2 Outpost reached 8.363 s.
Their raw results remain in the evidence. They are not credited as queue/device
improvements. The earlier v5 30.947 s install stall is likewise separate. The
v8 exploratory sidecar comparison encountered 40-43 s install spans and the old
idle socket deadline; it is not a valid sidecar speedup measurement. The source
of these off-CPU cache stalls is not established.

The same-binary L/P comparison isolates device routing. Showcase means improve
13.260 -> 11.441 s (13.7%). Outpost's first clean pair improves 33.018 -> 26.425 s
(20.0%); its pooled repeat is 24.615 s. Do not use the stalled second local case
to calculate the gain. Both pooled Outpost cases are below 30 s; Showcase remains
above 10 s. The original Outpost control mean is 34.951 s versus pooled 25.520 s,
an observed combined difference of 27.0%, not an isolated admission claim.
Repeated A/F walls are close here; the earlier admission ABBA and late-audio
latency evidence remains in `ara-asset/MEASUREMENTS.md`.

### Remaining stages

The first clean same-binary pair below gives summed elapsed spans in seconds
across concurrent workers, not an additive critical-path breakdown. LOD is
nested in mesh; GPU initialization and work overlap other stages. Cache store
includes publication I/O. `cache.sync` on memo reuse can include hardlink and
trace work, so it is not pure flush self-time.

| Stage | Showcase local | Showcase pooled | Outpost local | Outpost pooled |
|---|---:|---:|---:|---:|
| Assets-ready wall | 13.699 | 11.663 | 33.018 | 26.425 |
| Worker startup and input | 1.486 | 1.242 | 4.272 | 7.008 |
| GPU device initialization | 7.387 | 0.393 | 34.196 | 0.461 |
| glTF parse | 0.018 | 0.016 | 0.026 | 0.029 |
| Mesh processing, including LOD | 3.668 | 3.145 | 19.510 | 19.523 |
| LOD, nested in mesh | 3.145 | 2.558 | 12.412 | 12.320 |
| Texture decode and mips | 4.686 | 4.599 | 7.828 | 8.314 |
| Byte hashing (`hash.blake3`) | 0.602 | 0.577 | 1.322 | 1.313 |
| Action hashing (`action.hash`) | 0.324 | 0.274 | 1.921 | 2.275 |
| Cache store | 5.675 | 11.521 | 21.895 | 28.868 |
| Tracked import process CPU | 37.375 | 30.000 | 113.578 | 85.297 |
| Helper CPU, included above | 0 | 1.484 | 0 | 6.047 |

Across both repetitions, mean device initialization spans fall from 7.276 to
0.450 s Showcase and 33.795 to 0.460 s Outpost, with exactly two pooled devices
per case. Local import initialization excludes its separate warm-up query;
pooled initialization includes both shared devices. Wall times include warm-up
in both routes. Mean tracked import CPU, including helpers, falls 37.023 ->
30.766 CPU seconds Showcase and 111.344 -> 84.727 Outpost. CPU totals use the
last recorded process CPU counter for actual import workers plus helpers;
daemon, CLI and discovery/probe CPU are excluded.

Mesh/LOD is unchanged in the isolated Outpost pair, at about 19.5/12.3 summed
seconds. Decode/mips and variable cache I/O also remain. These are next profiling
targets; the measurements do not establish a hardware floor or justify weakening
durability. Optional `ARA_IMPORT_TRACE_CACHE_DETAIL=1` separates cache install
read/create/write/persist spans without changing cache behavior. The immutable
cache contract passes with it enabled. This checkpoint measures Balanced only;
the native correctness probe also covers Fast and HQ, without a new HQ wall claim.

### Correctness and lifecycle checkpoint

All 16 runs published 33/33 or 115/115 assets with zero fallback events. All
1,184 final action-key comparisons and 14,320 artifact references match the v4
baseline. Each helper start has a matching exit; every pooled case created two
devices. The opt-in native GPU protocol probe additionally checks six disposable
imports across all three presets against direct encoded bytes/keys, replacing
the original worker executable while the helper lives, and shutdown after the
last lease drops. The old executable-lock behavior fails with Windows error 32;
the runtime-copy implementation passes.

Idle sockets have no independent operation deadline: a worker may spend a long
time in CPU or cache I/O between encodes. Existing native-job watchdogs and
cancellation own that deadline. A scaled idle regression fails before the fix
and passes after it. Handshake/frame bounds and write deadlines remain. Tests
also cover authentication, malformed bulk requests, bounded device concurrency,
chunk tails, one-shot clients and executable/library snapshot invalidation.
The accepted Windows socket is explicitly returned to blocking mode.

Focused formatting and strict all-target Clippy pass for `ara-asset`,
`ara-texture-gpu`, `ara-import-worker` and `ara-importers`: 91 standard tests pass,
five are declared ignored, and three custom asset harnesses pass. The native
GPU probe above ran explicitly through the GPU lock. Blocking Compute resolve
and readback are unchanged. No full integration gate was run at this focused
checkpoint. Canonical per-run/per-asset evidence, hashes, outliers and verification
are in [`measurements/device-pool-2026-10-26.json`](measurements/device-pool-2026-10-26.json).
