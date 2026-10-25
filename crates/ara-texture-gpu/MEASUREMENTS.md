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
