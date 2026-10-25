# Asset import performance and quality

Measured 2026-10-23, Windows 11, Ryzen 7 7800X3D (8 cores / 16 threads), 32 GB,
Rust 1.99.1 MSVC. CARGO_BUILD_JOBS=2 for every Cargo command. Other production
worktrees remained active. No GPU, dev server, service, or system settings change.

## Release before / after

The merged implementation's in-process release baseline took **52,453.310 ms**
for DamagedHelmet (51,710.356 ms BC) and **4,857.604 ms** for Fox (4,795.544 ms BC).
It had one HQ preset, serial textures/mips/BC/meshes; Fast/Balanced did not exist.
The parent independently measured about 45 s for Helmet on this PC.

Current figures below are the median of three **fresh processes**, each with an
empty action/object cache and a private 16-thread Rayon pool. Cook includes
registry/pool startup, decode, geometry, mips, BC, container writing and validation.
Cold includes action hashing, lookup, cook and durable cache persistence. The
source GLB is read before the timer, matching the parent's registry.import test.
OS filesystem pages are warm; cold means an empty Ara cache, not an OS cache flush.
Warm is the median of ten checked cache loads per process, then the median across
three processes; hash/format validation is included, equality checks are outside
the timer. Columns are independently aggregated medians.

| Fixture / preset | Cook ms | Cook min-max ms | Cold ms | Warm ms | Peak RSS MiB (max) |
|---|---:|---:|---:|---:|---:|
| Helmet Fast | 281.926 | 281.488-363.028 | 676.965 | 11.987 | 509.3 |
| Helmet Balanced | 1,294.056 | 1,128.277-1,545.882 | 1,965.649 | 8.950 | 498.9 |
| Helmet HQ | 3,740.915 | 3,658.122-3,940.464 | 4,116.510 | 11.536 | 492.5 |
| Fox Fast | 30.460 | 27.338-35.949 | 128.824 | 1.368 | 34.1 |
| Fox Balanced | 44.477 | 38.744-67.867 | 123.768 | 1.481 | 35.6 |
| Fox HQ | 84.053 | 80.518-144.974 | 185.093 | 1.677 | 36.0 |

Helmet's 20.97 million base texels correspond to 74.4 MPix/s Fast / 5.61 MPix/s HQ
including scene/mesh work. Fox is 34.4 / 12.5 MPix/s. HQ cook speedups over the
merged baseline are 14.0x Helmet / 57.8x Fox, without reducing the selected preset.
Fast first generations are 186x / 159x faster than the previous HQ-only import.

One Fox Fast persistence run took 3.81 s (cold total 3.84 s), despite a 35.95 ms
cook; the other two cold runs were 125 and 129 ms. This is retained in the raw
samples. Preview publication now precedes disk persistence so such storage stalls
do not delay the visible asset. Final/export completion still waits for durable
cache storage. Cache I/O remains off the editor thread.

Peak RSS is Windows GetProcessMemoryInfo.PeakWorkingSetSize for the fresh process,
captured after cook/cache measurements and before independent quality decoding or
the optional serial recook. It includes source snapshots, float/mip/packed pixels,
artifacts and cache loads; excludes GPU allocations and other processes. Texture
preparation retains at most eight images per batch. The example reports 0 for
memory/CPU counters on non-Windows; those platforms have not been measured here.

## Elapsed stages, release, 16 threads

These are median **batch phase wall times**, not sums of overlapping task spans.
Texture preparation uses barriers between decode, mips and BC within each bounded
batch. Mesh includes attribute preparation, optimization, quantization and meshlets.
BC includes packing, solid-block reuse and KTX2 writing. Validation is registry
payload validation; checked cache-read validation is separately included in warm.
Small untimed JSON/bookkeeping work and pool creation account for the remainder.

| Fixture / preset | Decode/prep ms | Mips ms | BC ms | Mesh ms | Validation ms |
|---|---:|---:|---:|---:|---:|
| Helmet Fast | 112.446 | 37.706 | 88.077 | 48.669 | 0.190 |
| Helmet Balanced | 125.977 | 45.196 | 1,042.974 | 52.124 | 0.244 |
| Helmet HQ | 136.722 | 38.963 | 3,513.885 | 47.686 | 0.183 |
| Fox Fast | 11.977 | 2.140 | 12.239 | 1.990 | 0.319 |
| Fox Balanced | 11.565 | 2.504 | 26.462 | 1.482 | 0.325 |
| Fox HQ | 12.915 | 2.534 | 66.641 | 1.532 | 0.322 |

Rayon work is bounded by ImportInput.thread_budget, default 16. Native kernels
run on fixed four-pixel-high block-row tiles; mips, texture jobs and mesh/primitive
jobs are independent. Packing/filtering are parallel too. Pixel reduction order
is fixed. Repeated solid BC7 blocks reuse byte-identical ISPC results; Fox has
93.7% solid base blocks and Helmet's emissive map 97.1%. This is exact redundancy
elimination, independent of preset and thread count. HQ ISPC settings are unchanged.
Worker processes remain below-normal priority; registry benchmarks use normal
host priority, just like the serial baseline. In-process calls never run on editor
frames. Users can lower thread_budget without changing bytes or cache identities.

## Real worker progressive publication

Release ProcessWorker + HotReload, separate below-normal worker processes for
preview and final, empty cache, real IPC, validated boundary publication:

| Fixture | Preview Ready | Final Ready (cumulative) | Final after preview |
|---|---:|---:|---:|
| DamagedHelmet | 530.879 ms | 5,418.731 ms | 4,887.852 ms |
| Fox | 389.411 ms | 1,150.524 ms | 761.113 ms |

These are one end-to-end sample per scene, not the in-process cook medians above.
They include process launch, transport, background priority scheduling, inter-pass
cache persistence and final cache writes. All in-process cook targets are met;
Fox's final worker/publication overhead exceeds its 0.5 s cook target while the
first visible generation is under 0.5 s. Each worker result remains validated and
cached; both publication events honor the existing D20 boundary/Play policies.
Preview Ready precedes cache persistence, and the final cache hit bypasses previews.

## Quality per preset

Pinned Intel ISPC 0.5.0; independent BC7 decoder texture2ddecoder 0.1.2. RGB base-level
PSNR is 10*log10(255^2/MSE) over 8-bit source code values, excluding alpha. sRGB is
compared in its encoded domain. It measures codec distortion, not perceptual quality.

| Source / cooked usage | Fast dB | Balanced dB | HQ dB |
|---|---:|---:|---:|
| Helmet image 0, color | 45.513 | 48.399 | 48.732 |
| Helmet image 1, linear metallic/roughness | 39.903 | 44.717 | 44.834 |
| Helmet image 2, emissive color | 52.646 | 57.016 | 57.159 |
| Fox image 0, color | 53.109 | 61.213 | 61.477 |

HQ values match the merged implementation. Fast uses BC7 opaque/alpha ultrafast,
BC6H fast; Balanced basic/basic; HQ BC7 slow / BC6H very slow. BC4/BC5 are unchanged
across presets: generated 64-square fixtures measured 52.992 dB (R UNORM, peak 1)
and 58.770 dB (normalized normal XY, peak 1). Float32 BC6H decoding through
bcdec_rs 0.2.1 measured Fast 43.394, Balanced 43.360, HQ 43.128 dB on generated HDR
radiance (RGB, peak 8). A preset's search objective need not monotonically improve
PSNR on one synthetic image; these are checks, not a representative HDR corpus.
Negative, alpha-bearing and extreme HDR still retain float16/float32 fallbacks.
Normal/sRGB tests independently verify normalized and linear-light mip filtering.

## Reproduce

Khronos GLBs belong in .fixtures/ (gitignored; regular tests never use the network):

- [DamagedHelmet](https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb),
  SHA-256 a1e3b04de97b11de564ce6e53b95f02954a297f0008183ac63a4f5974f6b32d8.
- [Fox](https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Fox/glTF-Binary/Fox.glb),
  SHA-256 d97044e701822bac5a62696459b27d7b375aada5de8574ed4362edbba94771f7.

```powershell
$env:CARGO_BUILD_JOBS = '2'
cargo build --release -p ara-importers -p ara-import-worker --examples --bins
# Repeat each fixture/preset three times, fresh executable process each time:
target/release/examples/import_bench.exe crates/ara-importers/.fixtures/DamagedHelmet.glb fast 16
# Replace fast with balanced or hq, Helmet with Fox. --verify recooks at 1 thread:
target/release/examples/import_bench.exe crates/ara-importers/.fixtures/Fox.glb hq 16 --verify
# Actual below-normal worker processes, progressive publication and both caches:
target/release/examples/progressive_bench.exe crates/ara-importers/.fixtures/DamagedHelmet.glb target/release/ara-import-worker.exe
cargo test -p ara-asset -p ara-asset-format -p ara-importers -p ara-import-worker
cargo clippy -p ara-asset -p ara-asset-format -p ara-importers -p ara-import-worker --all-targets -- -D warnings
cargo fmt -p ara-asset -p ara-asset-format -p ara-importers -p ara-import-worker --check
```

All 18 canonical registry benchmark samples are in measurements/.
The worker samples are recorded alongside them as *-progressive.json. Full-size --verify runs
compare complete Helmet/Fox outputs at 1 and 16 threads for every preset. Generated
NPOT/alpha/BC4/BC5/BC6H fixtures also compare every preset across budgets. Focused
process tests cover crashes, deadlines, supersession, first-ready before cache
persistence, preview/HQ swapping, final-cache bypass, cancellation and failed HQ
last-good fallback under Play/idle/approval policies. Sidecar preset edits are typed
kernel transactions with undo, and schema/serde/Rust defaults all select HQ.
Mesh/skeleton/clip formats and runtime readers remain v1. See INTEGRATION.md.

## Deeper static LODs, 2026-10-25

Generation 6 reduces selected main-view triangles by **23.8–27.8%** in the
144-tile Outpost. Both runs use the same authored scene, 8,344 instances,
273 shared geometries, seed 7, look simulation tick, four look cameras,
2560×1440 resolution and 1 px projected-error threshold.

| Camera | Before main triangles | After main triangles | Reduction | Before casters ≤64 m | After casters ≤64 m |
|---|---:|---:|---:|---:|---:|
| gate | 2,848,843 | 2,056,685 | 27.8% | 1,678,587 | 1,484,289 |
| campfire | 2,414,267 | 1,831,979 | 24.1% | 1,450,341 | 1,200,105 |
| yard | 2,241,937 | 1,708,591 | 23.8% | 1,530,599 | 1,296,007 |
| vista | 3,502,478 | 2,587,588 | 26.1% | 1,414,946 | 1,299,888 |

The visual-quality agent's CPU replay runs on the shipping adapter's prepared
scene, before device creation. It reproduces frustum bounds and projected-error
LOD selection. The 64 m caster proxy falls 8.1–17.3%; it counts nearby candidate
triangles and does not measure actual cascades or GPU time. These results carry
no D26 timing or visual acceptance claim.

The normal `ara_play_host::assets::Assets` / `ProcessWorker` pipeline upgraded
101 tracked sidecars through typed version transactions and pinned all 115 assets.
All 452 mesh artifacts retain their LOD0 triangle counts, positive LOD counts,
finite nondecreasing errors and at least 25% reduction per accepted level.
103 chains gain levels; 3 of the 18 previously single-LOD meshes above 200
triangles gain a chain. The explicit HQ re-cook, pinning and artifact audit took
366.0 s with one worker and four threads per cook on the shared machine.
The generation-5 cache-hit baseline took 52.2 s; these are different cache states.

`namaqualand_rocks_02_d` extends from `[24910,12454,6226,3112]` to
`[24910,12454,6226,3112,1556,778,388]` (1.56% of LOD0).
Shrub `[2385,1191,863]` and rooibos stem `[1452,656,334,236]` levels retain
their geometric-error prefix. Weighted attribute errors apply to new levels;
retaining the prefix prevents finer runtime selections on existing foliage.

The requested 0.8–1.2M triangles per view is **not reached** with the specified
border and card safeguards. Rooibos C's 708- and 2,044-triangle leaf parts have
all 786 and 3,078 source vertices on geometric open borders, respectively.
They cannot collapse while those vertices remain locked. Thin connected cards
keep at least 90% area; rotated coincident backfaces are covered, while long
three-dimensional masked stems are classified separately.

Focused validation: 29 importer tests, strict all-target Clippy (including the
worker and benchmark example), and package formatting pass. Unchanged engine
libraries were reused from a frozen build; only owned code was recompiled.
Raw runs are under this worktree's ignored `target/ara-dev/lod/`:
`{before,after}-normal.json`, `{before,after}-triangles.log`, `comparison.json`
and `card-topology.jsonl`. Regular tests generate no sample recipes or captures.

For another normal re-cook and visual/timing follow-up, use a warm target with
matching sample and worker binaries:

```powershell
$env:CARGO_BUILD_JOBS = '2'
cargo build --profile perf -p ara-samples -p ara-import-worker
$tools = Join-Path $env:CARGO_TARGET_DIR 'perf'
$env:ARA_IMPORT_WORKERS = '1'
$env:ARA_IMPORT_WORKER = Join-Path $tools 'ara-import-worker.exe'
cargo run -p ara-dev -- gpu -- "$tools/ara-samples.exe" --samples samples capture outpost outpost --quality high
```

VQ's temporary `ARA_RASTER_TRIANGLES` replay hook can repeat the CPU counts at
`ARA_LOD_PIXELS=1`. To measure through `ara-dev perf`, export a **new** frozen
pack with the matching worker and select it with `--scene <export-folder>`.
Existing immutable packs keep their old geometry when source assets are re-cooked.
