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

## Cold import CPU checkpoint, 2026-10-25

An empty project cache, created with `ara hub create --template showcase|outpost`,
then opened through `ara hub open --headless`, published every asset through the
normal daemon `Assets` pipeline. Pinned source art and generated kits were present
before timing. OS file pages were warm; Ara action/object/intermediate caches were
empty. This is the real import path, not an encoder-only benchmark or shipped cache.
The Windows RTX 4070 SUPER host was shared with other agents; CARGO_BUILD_JOBS=2.

| Sample | Assets | Original HQ import | New default Balanced import | Create time (new) |
|---|---:|---:|---:|---:|
| Showcase | 33 | 129.784 s | 27.071 s | 2.266 s |
| Outpost | 115 | 250.668 s | 47.995 s | 3.607 s |

The original aggregate worker-stage times were 338.5 s compression / 354.4 s total
for Showcase, and 753.2 s compression / 848.7 s total for Outpost. Concurrent worker
stage sums exceed project wall time. Outpost mesh work was 43.7 s, including 22.2 s
LOD simplification; Showcase mesh work was 4.1 s. Decode/mips totaled 10.3 s and
25.0 s respectively. Hashing was 0.29/1.11 s; sidecar reads 0.001/0.007 s. Queue
and IPC-read time include waiting and must not be attributed to hashing or decoding.
The main cost is BC encoding. Existing tiled ISPC kernels saturate their bounded
Rayon pools; ~750 CPU-seconds for Showcase HQ imply about 47 s even with ideal
16-logical-thread utilization on this machine. That is a current-codec work floor,
not a lower bound on other algorithms or GPU hardware.

Changes bound retained decode/mip memory, deduplicate texture tasks before decode,
share content-addressed intermediate texture/geometry work between disposable
workers, transmit dependencies as MessagePack binary, reuse immutable object
payloads, and reserve admission for short discovery requests. Discovery/transport/
reaping keep normal scheduling priority so loaded native encoders cannot starve
control work. Intermediate memo writes are disposable; final object-before-action
publication remains durable. Three pre-default runs compared all final action keys,
manifests and object bytes against the original HQ baseline: all 33/115 assets and
331/1,459 artifact references matched. Those optimizations alone did not establish
a repeatable wall-time improvement on the contended host. The table above includes
the explicitly approved Balanced default and must not be called a same-mode speedup.

Quality is a reflected typed project policy, `ara.imports`, edited with
`ara do import.project.configure -p preset=highQuality`. New sidecars inherit it;
`import.configure` sets `preset` or resets `useProjectPreset=true`. Standalone
`TextureSettings.preset` and model `MeshSettings.texturePresets` override individual
textures (image URI or `#index`, FBX texture ID or relative filename). Overrides,
effective quality and importer ABI versions enter action/intermediate keys. Geometry
and other texture tasks are reused on a per-texture quality edit. No export-time HQ
re-encode is implemented; that remains a possible later improvement.

CPU quality QA used 142 distinct albedos and 135 normal contents across 286 semantic
paths. Worst sRGB RGB PSNR loss was Showcase boombox speakers: 62.495 HQ -> 59.863 dB
Balanced (2.633 dB); Outpost airduct parts02: 52.194 -> 51.539 dB (0.655 dB). Lowest
Balanced PSNR was denim, 39.205 dB vs 39.228 HQ. All 135 BC5 full mip chains were
byte-identical between presets. Inspected source/HQ/Balanced crops for boombox,
denim and airduct, plus decoded and neutral-lit normal plane/sphere sheets for camera
strap and rusted aircon: subtle extra albedo error, no preset-specific normal change.
These are decoded texture/diagnostic-light checks, not engine scene captures. Source
negative-Z normals explain some large baseline BC5 reconstruction outliers in both
presets. All three authored foliage cutout alphas exactly match their opacity maps.

Raw per-asset/stage JSON, cache byte-equality reports, commands and screenshots live
under the worktree's ignored `target/ara-dev/import-speed/`; codec QA scratch is in
the dedicated D: validation target. No user profile or tracked source asset is changed
by the measurements. GPU encoders and end-to-end GPU measurements are a separate
checkpoint; this CPU checkpoint does not meet the 20-50x per-mode request.
Focused checkpoint validation: 102 tests passed across ara-asset, ara-importers,
ara-import-worker, ara-play-host, ara-hub-service, CLI and editor asset tests; two
fixture/GPU tests were intentionally ignored. The admission fixture completed 28
cold imports at peak 4 (cap 4). Strict Clippy passed for the touched libraries,
unit targets and relevant integration suites; formatting and snapshot verification
passed with updates disabled after reviewing the reflected policy changes. Latest
main at this checkpoint: bae0034d. No full workspace gate was run.

## Headless GPU worker checkpoint, 2026-10-25

These are single fresh-project runs through `ara hub create`, then
`ara hub open --headless`, ending when every expected asset is published by the
daemon. Each run starts with empty Ara action/object/intermediate caches and an
empty engine-profile Vulkan pipeline cache. OS file pages and the driver's own
disk cache are not flushed. Art/kits are already downloaded/generated. Other
agents and the main integration build remained active. The GPU lock's queue wait
is outside the timer. All CLI/worker/player executables are owned copies under
`D:/AraBuild/ara-dev-infra-target`; none comes from main's target.

| Sample | Assets | Verified owned ISPC HQ import s | GPU Balanced import s | GPU HQ import s | Create-through-ready Balanced / HQ s |
|---|---:|---:|---:|---:|---:|
| Showcase | 33 | 91.768 | 11.484 | 21.166 | 13.294 / 22.620 |
| Outpost | 115 | 159.061 | 34.441 | 47.452 | 37.794 / 50.670 |

The owner separately measured about **135 s** for cold Outpost before the editor
was usable in installed preview 13. The new 34.441 s result is the daemon's
assets-ready boundary, not a measurement of total interactive editor startup.
The earlier 250.668 s contended development baseline is retained above as history,
not used as the installed-build headline baseline. Balanced Outpost meets 45 s;
Showcase remains above 10 s, and HQ Outpost remains above 45 s.

Against the verified owned ISPC HQ runs, same-quality GPU HQ is 4.34x faster for
Showcase / 3.35x Outpost. The new default Balanced is 7.99x / 4.62x faster, including
the approved preset change. **The requested 20-50x wall-time acceleration is not
established**, nor is sub-second encoding of the whole Showcase set. These runs
do not prove an irreducible hardware floor; device startup, CPU preparation and
full integer candidate searches remain opportunities.

| Sample / backend | Import-worker CPU seconds | Sum mesh elapsed s | Sum decode s | Sum mips s | Sum compression elapsed s |
|---|---:|---:|---:|---:|---:|
| Showcase ISPC HQ | 768.969 | 3.542 | 4.295 | 2.984 | 240.125 |
| Showcase GPU Balanced | 34.531 | 3.191 | 2.598 | 1.158 | 21.733 |
| Showcase GPU HQ | 36.781 | 2.582 | 2.595 | 1.140 | 54.033 |
| Outpost ISPC HQ | 1371.828 | 21.704 | 7.448 | 5.038 | 444.643 |
| Outpost GPU Balanced | 105.750 | 18.342 | 5.213 | 2.129 | 56.895 |
| Outpost GPU HQ | 105.344 | 17.143 | 5.180 | 2.048 | 114.769 |

CPU seconds sum the last recorded process CPU counter for each actual import
worker, excluding daemon, CLI, discovery/probe workers and other agents. Stage
columns are elapsed spans across concurrent workers and nested Rayon work, not
additive CPU self-time or a critical-path decomposition. LOD spans are nested in
mesh; GPU packing/device initialization/wait are nested in compression. Artifact
validation is separate. Worker count is bounded to four, with four Rayon threads
per import; queued metadata gets admission priority. N-of-M publication remains
33/33 and 115/115. Balanced recorded 20/98 workers with nonzero GPU encode work;
HQ recorded 20/99. Every run had zero backend-fallback events.

The previous verified GPU implementation took 30.560 / 50.486 s Balanced. Its
profile showed multiple workers spending about 5 s creating the same cold kernels
and texture-free models initializing devices. The current worker negotiates and
executes one tiny BC7 warm-up, drops it before replying, then shares its engine
pipeline cache. Each disposable asset worker creates Vulkan only on first encode;
memo hits and texture-free models avoid it. In the current Balanced runs the
warm-up was 0.458 / 0.407 s; maximum per-worker pipeline creation was 20.5 / 29.2 ms.
This is a measured combined implementation/run difference, not an isolated
driver-cold A/B attribution to warm-up alone.

Remaining Balanced spans include 6.535 / 32.048 s summed device initialization,
3.256 / 12.313 s durable cache stores (including 2.033 / 7.550 s cache sync), and
0.570 / 1.249 s hashing. Per-process GPU encode timestamp spans summed to
13.117 / 19.088 s Balanced and 45.335 / 77.508 s HQ. Different device queues can
overlap on the shared GPU: those sums are not serial GPU time or a hardware floor.
Hashing and sidecar reads are small; removing them cannot supply the missing
20-50x wall speedup. Outpost's final audio asset alone took 5.294 s in its worker.
Showcase's longest Balanced assets were camera_01 (3.612 s), FlightHelmet
(3.402 s), SciFiHelmet (2.996 s) and vintage radio (2.926 s).

The checked-in `measurements/gpu-v4-*.json` records contain binary hashes,
aggregate stages and per-asset stages/CPU counters for these explicit benchmark
runs. Full raw traces, publication keys and commands remain under the worktree's
ignored `target/ara-dev/import-speed/{gpu-v4-balanced,gpu-v4-hq}-*` directories.
`ARA_IMPORT_TRACE_DIR` enables the same opt-in tracing for another real daemon
run. `ARA_TEXTURE_ENCODER=cpu` selects the qualified fallback for a control.
Use a fresh private `ARA_CONFIG_DIR`/`ARA_CRASH_DIR`, point `ARA_IMPORT_WORKER` and
`ARA_PLAYER` at matching owned binaries, and wrap timed imports in `ara-dev gpu`.

GPU algorithm timings, full-texture PSNR/visual checks, the normal-map CPU policy,
determinism scope and evidence are in
[`ara-texture-gpu/MEASUREMENTS.md`](../ara-texture-gpu/MEASUREMENTS.md). This work
ships shader/worker support, not pre-imported template content caches or an
export-time HQ re-encode. No D26/D27 runtime acceptance claim follows from import
measurements.

The v3/v4 Balanced production-cache comparison matched artifact names, kinds and
content hashes for all 33/115 assets and 331/1,459 artifact references. Action keys
can differ because the immutable codec cohort identity changed. The post-merge
unified Compute test and production-worker protocol test both passed, including
blocking resolve/readback, scalar/dot integer-oracle equality and clean Vulkan
validation after merging the deferred-completion API.

Focused checkpoint verification: 88 standard core tests and both custom worker
harnesses pass, including 28 cold imports at peak 4/cap 4. Four editor import and
progress unit tests, the typed import-settings/undo integration test and the CLI
reflection snapshot pass. The snapshot updates exactly three preset doc strings,
then passes with updates disabled. Formatting and strict all-target Clippy cover
the touched packages; no full workspace integration gate was run. Five core tests
are declared ignored in the normal run; the two GPU probes above ran explicitly.

## Device reuse and sample versions: 2026-10-26

The same-binary local/pooled follow-up measures Showcase at 13.260 -> 11.441 s
mean, and Outpost at 33.018 -> 26.425 s in the first clean pair, with a second
pooled Outpost run of 24.615 s. Two control I/O stalls remain explicitly marked
in the full A/F/L/P/P/L/F/A matrix; they are not credited as improvements.
These are fresh daemon assets-ready timings, not interactive installed editor
startup. Showcase still misses 10 s. Full stage sums, lifecycle tests and canonical
per-asset evidence are in
[`ara-texture-gpu/MEASUREMENTS.md`](../ara-texture-gpu/MEASUREMENTS.md#leased-device-reuse-2026-10-26).

Dedicated sidecar commit `21de76f5` refreshes 111 Outpost and 33 Showcase version
strings, with no settings or other data changes. Expected versions come from the
installed registry; `cargo test -p ara-importers --test sample_versions` detects
future skew without writing tracked files. `ARA_UPDATE_DEMO=1` explicitly updates
them through typed sidecar edits and the canonical writer. The test fails before
regeneration, then passes with updates disabled. Current sidecars reduce source
snapshot work (132/452 to 66/230 per sample); no isolated metadata wall-speedup
claim follows from the failed exploratory comparison.

## Verified cache publication: 2026-10-26

Checkpoint `757e458b` removes per-object/action filesystem flushes. Objects are
atomically published before actions. Every read verifies all referenced lengths,
content hashes and payload formats before returning a result. Missing or torn data
is a miss; repair atomically replaces damaged files under a per-entry OS lock.
Readers never unlink a racing repair, and different verified results for one key
remain nondeterminism errors. Import keys, compression and artifact bytes are unchanged.

The store-only Showcase replay (33 actions, four threads) fell from 6.124 s to
0.291 s initially and 0.439 s in the final replay. The control's 332 flushes used
16.317 summed wall seconds but only 0.438 thread CPU seconds. Its writes used
0.187 summed wall seconds. This attributes the control's wait to flushes.
The new Outpost replay still took 10.226 s at 100% machine CPU: writes alone used
26.869 summed wall seconds versus 0.234 thread CPU seconds. Retain that I/O outlier;
removing flushes does not remove every storage wait. Defender was active, but its
recorder was denied with `0x80070005`; these runs do not attribute waits to Defender.

The real daemon cold-cache ABBA below used the same source/profile/roots and one
GPU lock, with fresh project/import/driver caches. OS/vendor caches were retained.
Batch-27 builds were active. There was no owned build or stress load during timers.

| Order / cache policy | Showcase assets ready (s) | Outpost assets ready (s) |
|---|---:|---:|
| A1 / per-file flush | 21.235 | 22.198 |
| B1 / verified atomic publication | 18.085 | 22.109 |
| B2 / verified atomic publication | 11.077 | 23.280 |
| A2 / per-file flush | 10.263 | 26.087 |

This loaded matrix establishes no reliable end-to-end speedup or sub-10-second
Showcase result. The control itself varied twofold. B1 Showcase retained a 3.593 s
maximum cache-install wait; A2 Outpost retained 51.513 summed cache-store seconds.
All 592 action keys and 7,160 artifact references match v4; a post-timer physical
read verifies 5,344,011,560 logical referenced bytes across the eight caches.
No CPU fallbacks occurred, and helper starts/exits balanced with two devices per run.

Recovery checks kill writers during object writes, between object/action publication,
after action publication and while repairing under the OS lock. Explicit torn and
same-length lost-write injections cover failures a process kill alone cannot induce
while the OS cache survives. The final stress passed 200 concurrent repairs and
20 recovery loops (80 killed writers) in 29.995 s under the integration load.
Exploratory sharing-denial failures remain in the ignored logs; the retained fix
retries transient Windows sharing/access denials and only accepts a live replacement
after exact verification. Permanent denied publication still fails.

Raw evidence: `target/ara-dev/import-speed/cache-policy-summary.json`,
`cache-policy-{a1,b1,b2,a2}-balanced-{showcase,outpost}/`,
`cache-policy-verification.log`, and `cache-diagnostic/`.

## Device-free warm import metadata: 2026-10-26

Dependency discovery remains outside the editor/daemon. Concurrent cold requests
share bounded one-shot worker batches; verified memos cover importer/version,
source bytes, settings and worker arguments. Warm metadata hits start no discovery
children. Settings/source/version changes invalidate metadata without changing the
final action identity. Unsupported workers fall back to individual requests.

A verified capability memo covers the immutable codec cohort, ordered adapter
UUID/LUID identifiers, driver version/UUID and explicit Vulkan ICD selection.
Looking it up creates a Vulkan instance only, never a device or encoder service.
Unrelated worker rebuilds do not invalidate it. A cache miss prepares the encoder
and confirms its identity; a mismatch retries with the CPU key. The daemon shares
this hint with source/cook workers through its existing version-metadata request.
No template content caches are shipped or copied, and no codec bytes changed.

The frozen cached Outpost has 115 sources. Under batch-28 load, the eager control
needed 119/121 worker launches, 2.651/4.899 s of encoder preparation, and
18.431/10.523 s from the first source snapshot to final publication. It ran no
imports. An intermediate two-control-worker implementation had zero encoder
services/prepares/imports and unchanged keys: 5.237/1.081 s to settle. The first
retains a 3.323 s cache-read wait; the repeat's identity lookup cost 0.211 s and
one worker's spawn/queue span cost 0.479 s. These motivated combining the hint
with versions in one control child. Hub launch is a separate boundary: the
intermediate runs took 7.298/4.039 s including daemon/player startup.

The final A/B/B/A uses the existing cached project, profile, codecs and cache policy;
only metadata startup differs. Each run closes its daemon and waits for encoder
helper exit. No owned compilation runs during the GPU-locked matrix.

| Run | Source snapshot to settled (ms) | Version request to settled (ms) | Hub open to settled (ms) | Worker children | Encoder services |
|---|---:|---:|---:|---:|---:|
| A1 / eager control | 79116.405 | 79704.625 | 83425.214 | 121 | 1 |
| B1 / combined cached metadata | 385.875 | 905.070 | 2182.337 | 1 | 0 |
| B2 / combined cached metadata | 400.764 | 676.855 | 1352.502 | 1 | 0 |
| A2 / eager control | 9434.768 | 9720.362 | 11911.029 | 121 | 1 |

All four publish exactly 115 unchanged v4 action keys and run zero imports. Both
candidate runs have zero encoder preparation and zero discovery batches. The
source/cache work is below 0.5 s, while total import initialization including the
one control process and validated identity lookup remains 0.677–0.905 s. Do not
describe that latter boundary as below 0.5 s, or as editor first-frame acceptance.
Candidate identity lookup takes 248/193 ms; peak individual cache loads take
150/150 ms. These spans overlap other work and must not be summed as a stage total.

Keep A1's 51.158 s cache-read wait and 21.301 s encoder preparation as outliers.
A2's preparation takes 7.642 s. No speedup ratio is based on the 79-second control.
Batch-28 is active; sampled machine CPU before A1/B1/B2/A2 is 23/49/0/19 percent,
which is neither an interval average nor evidence of quiet storage. The checked-in
`measurements/warm-*.json` records preserve counters, stage spans and binary hashes;
raw traces/results are under the matching ignored `target/ara-dev/import-speed/`
folders. `measurements/cache-policy-2026-10-26.json` retains the earlier cold matrix.

Focused tests exercise batch/legacy compatibility, source/settings invalidation,
torn metadata repair, zero device preparation on a cache hit, one preparation on
the first miss and CPU-key retry after an identity mismatch. The production owner
uses the same worker for metadata, snapshots and cooks; remote/custom workers keep
the original fallbacks. No importer version bump or sample-sidecar rewrite is needed.

Final validation also runs the ignored unified Compute integer-oracle probe and
the production GPU-worker protocol probe through the GPU lock. The latter forces
six real encodes across all presets by disabling intermediate texture memo reuse
after capability negotiation; otherwise memo hits correctly have no encoder PID.
After the service exits, a fresh worker recovers the capability hint without a new
service. Physical verification reads all 115 actions, 1,459 artifact references and
909,360,516 logical referenced bytes successfully. The fixture's initial memo-hit
assertion failure remains in the ignored logs; no production codec fix was needed.

## Bounded parallel geometry: 2026-10-26

The subsequent eight-case fresh-project ABBA does not confirm a cold-import win:
Outpost open-to-ready means regress from 28.567 s to 40.454 s (all runs retained).
For preview 16, `mesh::PARALLEL_GEOMETRY = false` restores sequential LOD0 solves
and classic/virtual cooks by default. Re-enable only after an end-to-end ABBA win.
The diagnostic results below describe the experiment, not the shipping default.

The fresh Hub-created Outpost profile `cold-mesh-profile-a0-outpost` uses the real
daemon pipeline, default Balanced, a fresh project/import/profile/driver cache and
the machine-wide GPU lock. It publishes all 115 assets in **55.556 s after open**,
or **60.439 s including project creation**. All 115 action keys and 1,459 artifact
references match v4, with 97 GPU-backed imports and zero CPU fallbacks. This is a
loaded storage outlier, retained rather than used as a speedup baseline. Available
RAM falls from 4.67 to 3.98 GiB; point samples of machine CPU are 12/20 percent,
not an interval average or evidence of quiet storage. No owned build runs during
the timer. Ordinary filesystem/vendor caches are retained.

| Stage | Summed elapsed seconds |
|---|---:|
| Parse | 0.025 |
| Mesh, including classic and virtual geometry | 19.643 |
| Classic LOD, nested in mesh | 12.086 |
| Image decode | 7.059 |
| Mips | 3.035 |
| Compression, including GPU/service waits | 35.541 |
| Device creation, helper total | 1.028 |
| BLAKE3 hashing, all traced calls | 1.591 |
| Cache store | 660.328 |
| Atomic installation, nested in store | 630.509 |
| Persist/close, nested in installation | 444.689 |
| Writes, nested in installation | 137.049 |

Stages overlap across workers and have nested spans; they must not be added into
a critical path. Total tracked worker/helper CPU is 74.484 s. There are 51 cache
installation spans above five seconds (maximum 9.993 s). The last asset waits
47.891 s for admission. These waits explain why this loaded run cannot establish
the cold targets or an isolated mesh speedup.

A diagnostic now separates the final writer close from persist. A four-action,
four-thread store-only replay verifies its outputs in 116.244 ms: 29 writes total
6.527 ms, attributes/atomic renames 117.854 ms (maximum 101.306 ms), and closes
0.957 ms (maximum 0.060 ms). This small replay does not reproduce the long storage
waits. `tempfile` uses `SetFileAttributesW` followed by `MoveFileExW` without
`MOVEFILE_WRITE_THROUGH`; these results do not attribute the outlier to Defender
or justify another cache-policy change.

Geometry can be isolated without encoding or storing. The explicit diagnostic
example `cargo run -p ara-importers --example geometry_replay -- PROJECT 4` reads
the completed project's verified texture memo and backend identity, forces classic
and virtual geometry to cook, and compares each entire `ImportOutput` literally
against the production action. It fails if any texture needs encoding, if settings
or versions are stale, or if an output byte differs. Dependency discovery runs in
this standalone diagnostic process, never in the editor. This replay is not a
fresh-project end-to-end measurement.

All three diagnostic variants below verify the same 101 registered glTF models
from the fresh Outpost project. Each uses four worker threads, the same dev profile
(own code opt-level 1, dependencies 3), a GPU lock and no owned compilation during
the timer. Other agents remain active; these are single diagnostic passes.

| Summed interval (s) | Original order/maps | Parallel scheduling | Plus topology lookups |
|---|---:|---:|---:|
| Model cook | 19.073 | 12.771 | 12.531 |
| Mesh | 15.848 | 11.406 | 10.786 |
| Classic LOD | 9.769 | 8.830 | 8.409 |
| Topology/attribute preparation | 1.712 | 1.865 | 1.480 |
| Native simplifier calls | 7.747 | 8.793 | 8.557 |
| Card-area checks | 0.022 | 0.018 | 0.020 |
| Virtual geometry | 5.875 | 6.564 | 6.504 |

The implementation evaluates LOD0-based targets in pairs within the existing
bounded pool, then applies acceptance, geometric/attribute fallback, area checks,
monotone errors and the goal stop in original order. A one-thread budget evaluates
one candidate at a time. At most one speculative candidate is unused. glTF classic
and virtual cooks overlap, with the same immutable inputs and error precedence.
Topology maps use a fixed hasher: IDs are assigned in source order; unordered
iteration only ORs border flags. Floating-point reductions keep triangle order.
No simplifier parameters, compression, importer versions, keys or artifact formats
change. More simultaneous native work can increase summed simplifier/virtual
intervals through preemption and overlap; these sums are not thread CPU time.

The final source after syncing main repeats all 101 exact production-output
comparisons (1,441 artifact references) at one and four threads. The replay's
summed model cooks are 23.932 s and 11.038 s respectively. Both reuse all textures;
neither creates a device or encodes. They are additional correctness/diagnostic
passes, not a new controlled baseline/candidate wall comparison.

The focused checkpoint runs formatting, strict all-target Clippy and all ordinary
tests for `ara-asset`, `ara-importers` and `ara-import-worker`, plus the explicitly
GPU-locked production-worker protocol test (six actual encodes and lease cleanup).
There are 69 ordinary libtest passes, three declared ignores in that run, and
successful custom worker-limit/fairness harnesses; the GPU ignore runs explicitly.
LOD tests compare complete static and deformed outputs at caps 1/4/8 across private
pools of one/four threads; existing seam, area, depth and sample-version tests pass.
The main-sync shader cohort is re-cooked with all artifacts reused unchanged.

The controlled fresh-project wall matrix follows below. Neither the diagnostic
reduction nor the retained 55-second storage outlier establishes Showcase below
10 s or Outpost below 20 s.
Canonical evidence is in `measurements/parallel-geometry-2026-10-26.json`; raw
traces, per-model outputs, frozen worker hashes and logs remain under
`target/ara-dev/import-speed/` (`cold-mesh-profile-a0-outpost/`, `geometry-*`,
`cache-close-a0/`, `mesh-final-*`, `mesh-gpu-protocol/`).

### Fresh-project ABBA and regression attribution

Eight cases run through the real Hub/daemon pipeline under the GPU lock, with
fresh project/import/profile/driver caches and the same frozen CLI/player/profile.
A is the matched `63e07a1d` control worker; B is `939cb781`. Other agents are held;
the installed preview may remain open and idle. OS/vendor caches are retained.

| Run | Showcase open to ready (s) | Outpost open to ready (s) | Showcase including create (s) | Outpost including create (s) |
|---|---:|---:|---:|---:|
| A1 | 12.324 | 28.403 | 14.165 | 38.353 |
| B1 | 13.401 | 34.035 | 15.732 | 42.799 |
| B2 | 10.775 | 46.872 | 12.371 | 51.052 |
| A2 | 11.275 | 28.732 | 12.582 | 35.956 |

Retain all cases: mean Outpost regresses 28.567 to 40.454 s; Showcase is 11.800
to 12.088 s. The runner exhausts PATH length before A2 by repeatedly appending
native paths. Only its environment-path deduplication is corrected, then A2 resumes
under the lock; no completed case is discarded or repeated. All 592 action keys
and 7,160 artifact references match v4. Physical verification reads 5,344,011,560
logical referenced bytes, with zero CPU fallbacks. The owner subsequently deletes
the eight scratch projects after reparse checks; raw traces, catalogs and IDs stay
under this worktree's `target/ara-dev/import-speed/parallel-lod-*` on C:.

| Outpost interval/counter | A1 | B1 | B2 | A2 |
|---|---:|---:|---:|---:|
| Native worker CPU (s) | 64.391 | 64.719 | 64.781 | 63.250 |
| Mesh elapsed sum (s) | 15.609 | 10.379 | 10.118 | 14.938 |
| Classic LOD elapsed sum (s) | 9.764 | 7.971 | 7.805 | 9.176 |
| Cache-store elapsed sum (s) | 72.745 | 104.949 | 258.034 | 61.063 |
| Cache-write elapsed sum (s) | 33.850 | 28.383 | 99.510 | 10.822 |
| Cache-write thread CPU (s) | 0.250 | 0.250 | 0.250 | 0.203 |
| Longest cache install (s) | 4.541 | 6.401 | 8.623 | 2.463 |
| Installs over one second | 8 | 7 | 22 | 4 |
| Largest worker peak RSS (MiB) | 444.0 | 443.6 | 444.9 | 430.2 |

Every worker uses four pool threads; the experiment adds tasks, not another pool.
The observed cost is off-CPU filesystem waits: B1 includes 6.2–6.4 s link/read/write
stalls, while B2 has several worker/daemon writes ending almost together after
about 8.5 s. B2's last asset starts 41.725 s after open; its import takes 4.548 s,
with only 0.469 s of process CPU. Both controls retain smaller I/O stalls too.
There are 667 store and 2,477 install spans in each Outpost case, so this is not
an increase in published content. Substage intervals overlap and nest; do not sum
them into a critical path. Thread CPU counters are coarse, and individual process
RSS peaks are not a simultaneous system-memory measurement.

These traces locate the added wait but do not prove why the filesystem stalls.
Changed arrival/order and storage pressure remain hypotheses; there is no recorded
system fault/queue telemetry or Defender attribution. The shipping default is
restored by `40cc47a7`; re-enabling requires an end-to-end ABBA win. Canonical
evidence: `measurements/parallel-geometry-abba-2026-10-26.json`.

## Byte-exact Color8 preparation: 2026-10-26

The existing decode timer includes entropy decoding, serial RGBA32F expansion and
sRGB transfer. PNG/JPEG/HDR/EXR are supported; there is no DDS/KTX source-import
path to bypass for these samples. The measured change fuses eight-bit color
expansion and transfer into one parallel destination. A 256-value table is built
through image's conversion for the actual L8/LA8/RGB8/RGBA8 layout and the unchanged
sRGB function. A direct u8/255 table initially fails one grayscale float bit;
that failed probe log is retained. Small images below 256 pixels, other sample
types, normals, masks, linear data and HDR use the qualified original path.
There are no new dependencies, formats, versions, cache inputs or codec changes.

The standalone probe compiles the production helper source, including cancellation
and destination allocation. It runs A/B/B/A per semantic image over 147 albedo
paths (229,403,604 base pixels), with two Rayon threads under the GPU lock and
batch-32 gate load. This corpus includes repeated content, not 147 unique models.
Every decoded base float bit matches; no encoding, device creation or cache store
runs. The table below averages each variant's two preparation passes per image,
then sums images; it is not a fresh-project import wall measurement.

| Corpus phase total (s) | Own code opt1 | Own code opt3 |
|---|---:|---:|
| Reference entropy decode | 1.380 | 1.480 |
| Reference RGBA32F conversion | 0.680 | 0.689 |
| Reference transfer and validation | 3.563 | 3.391 |
| Reference preparation, complete | 4.248 | 4.084 |
| Production lookup preparation | 1.806 | 1.389 |
| Preparation speed ratio | 2.35x | 2.94x |
| Original mip generation, one pass | 1.417 | 1.292 |

The earlier lighter prototype records 3.35x/3.60x; it omits the production
cancellation/validation and buffer policy and is not the shipping-path result.
Neither phase result establishes the cold targets. No mip or compression algorithm
changes. Focused tests check all code values/four layouts/odd tails at one and four
threads, literal complete KTX2 mip bytes against the pre-table path, cancellation,
all preset/thread contracts and sample versions. Formatting, strict all-target
Clippy and all 38 importer tests pass, with none ignored. The main-sync shader
re-cook reuses every artifact unchanged.

Canonical evidence: `measurements/texture-color-lookup-2026-10-26.json`.
Raw probe inputs, source/library hashes, both failed/successful logs and all timing
rows stay under `target/ara-dev/import-speed/texture-prepare/`. Fresh-project
before/after remains a separate measurement; parallel geometry stays disabled.

## Memory/device pressure diagnostic: 2026-10-26

A single unchanged sequential-control Outpost import runs in the owner's idle
window under the GPU lock. It publishes 115 assets in 28.511 s after open,
32.173 s including create. All keys and 1,459 artifact references match v4;
physical verification reads 909,360,516 referenced bytes. This is a diagnostic,
not another A/B matrix. The owner deletes its scratch project after independent
reparse checks; all evidence is extracted to C: first.

The counter sampler records 62 samples, 26 during import, before/during/after
the job. System/disk counters have no invalid samples. Maximum gap is 1.851 s;
maximum process/system snapshot offset is 55.6 ms. All nine PIDs associated with
the ten writes over one second are captured. Short-lived processes can be missed,
so sampled logical process-I/O totals are incomplete. The sampler uses 2.516
CPU seconds across 64.633 sampled wall seconds (about 0.24% of 16 logical CPUs).
Its provider initialization happens before the measured import.

| Active-import measurement | Range/result |
|---|---:|
| Available physical memory | 5.841–7.378 GiB |
| Standby cache | 5.827–6.726 GiB |
| Modified-page list | 125–417 MiB |
| Commit charge / limit | 52.8–57.1% |
| Pages Output/sec and Page Writes/sec | zero throughout all 62 samples |
| Longest cache write | 2.410 s |
| D:/E: maximum sampled average queue | 10.32 |
| D:/E: maximum sampled average write latency | 340.4 ms |

During the six counter intervals overlapping slow writes, available RAM is
6.179–6.545 GiB, modified pages 125–254 MiB, and Pages Output/sec remains zero.
Mean disk queue is 5.66 in those intervals versus 4.22 outside them. The largest
average-write-latency spike is outside those slow-write windows, so these figures
are not proof of a specific causal layer. The previous 8.6-second freeze does not
reproduce. This quiet run does not support a RAM-shortage/modified-page-writer
explanation; it does not exclude buffered-file-cache writeback, filters or device
behavior. No cache throttling or publication policy changes follow from it.

Windows reports D:/E: on one Lexar NS100 2 TB SATA SSD, and C: on a separate
T-FORCE NVMe SSD. The pagefile is on C:. Process I/O is logical, can include other
volumes, and may count the same data again in System writeback; it must not be
equated with physical disk throughput. Missing first/short-process rate samples
are not evidence of zero I/O. Raw counter JSONL, the sampler source, correlation
CSV, traces and source/action catalogs are retained under this worktree's
`target/ara-dev/import-speed/pressure/pressure-diag-01/` and
`pressure-diag-01-outpost/`. Canonical summary:
`measurements/cache-pressure-2026-10-26.json`.

The process-counter self-test deliberately launches a new short-lived process.
An initial `Get-Counter` process-wildcard query misses it because its instance list
is fixed. The retained sampler instead refreshes `PerformanceCounterCategory`
data per tick; the self-test then observes the new PID. Static system/disk
counters retain their continuous query. This instrumentation creates no service
or watcher after its bounded run.

### Default project/cache placement

The native Hub's `home()` starts at `USERPROFILE/Documents` if that literal
directory exists, otherwise `USERPROFILE` (`HOME` fallback). The form appends the
project-name slug and allows a different destination. CLI/MCP `hub.create` require
an explicit path. On this PC, the Hub default is on C: NVMe; this is not a claim
about every tester's drive. It is a literal Documents path, not Windows Known
Folder redirection. The default content cache is `<project>/.ara/cache`, including
its intermediates, and follows the chosen project volume. `ARA_CONFIG_DIR`
controls the profile, not this content-cache placement. Hub copies template
sources while excluding `.ara`, `.git`, `target` and `evidence`.

The earlier D: placement was an explicit benchmark choice. A four-case NVMe
Outpost A/B/B/A is prepared with the same frozen workers/CLI/player, projects and
caches on `C:/AraImportBench/projects`, and Cargo/binaries still on D:. It awaits
the next quiet slot; no NVMe win or adaptive-write policy is established yet.

## NVMe geometry ABBA: 2026-10-26

The subsequent quiet slot used the same frozen workers, CLI, player and dev
profile as the SATA pairing, with fresh Hub-created Outpost projects and their
content caches at `C:/AraImportBench/projects`. One GPU lease covered all four
runs and physical verification. Open-to-assets-ready seconds, in order:
**A1 27.5219161, B1 19.5642159, B2 19.2434134, A2 23.3599563**.
Sequential mean **25.4409362 s**, parallel mean **19.40381465 s**: **23.7% lower**.
Create-through-ready seconds are **30.6671606 / 22.6523806 / 21.2705094 /
25.9346803** (means 28.30092045 / 21.961445). Every result is retained; the A1/A2
difference remains visible. Fresh content caches do not imply flushed OS/driver
caches. These frozen workers precede the Color8 and mip improvements below.

All 460 action keys and 5,836 artifact references match v4. Physical verification
checked 3,637,442,064 referenced bytes; each run publishes 115 assets with zero
fallbacks. Evidence and exact binaries: `measurements/parallel-geometry-nvme-2026-10-26.json`.
Raw traces/catalogs/source IDs/results remain under
`target/ara-dev/import-speed/nvme-parallel-{a1,b1,b2,a2}-outpost/`.
The owner deleted all four projects after independent reparse checks.

NVMe gains contrast with the SATA pairing's **28.56711525 -> 40.4538923 s**
(41.6% slower). Parallel geometry stays disabled until adaptive cache-write
admission is validated on both devices; this experiment does not validate an
adaptive policy or identify the device's internal cause.

## Byte-exact color mips: 2026-10-26

For power-of-two color textures, a half-size mip has exactly four unit-weight
samples. Specialize their coordinate/weight calculation while preserving sample
order, premultiplied-alpha sums and the original alpha cutoff. Levels still depend
on their rounded predecessor. NPOT, one-axis, normal, data and HDR levels keep
the original area filter verbatim. No codec, version, key or sidecar changes.

The production module was compiled into the standalone full-chain differential
probe and run under `ara-dev gpu` with two Rayon threads, per-texture A/B/B/A.
The batch-33 gate was active: CPU snapshots 3% before / 7% after, with Cargo and
later rustc/linker processes present. These are phase measurements, not quiet-window
fresh-project walls. All 147 semantic albedo paths (142 unique input hashes),
229,403,604 base pixels and 76,466,837 generated mip pixels retain every float bit
in both profiles. All repeats and per-texture times are retained in
`measurements/texture-mips-2026-10-26.json`.

One corpus pass, averaging the two repetitions per variant: dev/opt1
**1.3070384 -> 0.26782325 s (4.88x)**; opt3
**1.15200885 -> 0.2220697 s (5.19x)**. The earlier prototype's 5.01x/5.14x results
are not substituted for these production-source results. No cold-import-wall
claim follows yet. Raw scripts, provenance and logs remain under
`target/ara-dev/import-speed/mips/`.

Focused fmt, strict all-target Clippy and all **41 importer tests** pass, with no
ignored tests. Differential tests cover complete chains at one/four threads,
maximum-axis dimensions, transparent/tiny-alpha pixels, all usages and arbitrary
area-filter fallback sizes. Complete POT/NPOT KTX2 bytes match the original filter
in Balanced/HQ; sample-version checks pass without regeneration.

## Current cold Outpost and rejected cache admission: 2026-10-26

The current **sequential** NVMe control imports all 115 Outpost sources in
**20.099653 / 19.8313339 s**, mean **19.96549345 s**, through fresh Hub-created
projects and the real daemon/worker publication path. It includes the Color8 and
byte-exact mip improvements. Create-through-assets-ready is **23.4508058 /
21.3755066 s**, mean **22.4131562 s**. This is approximately **20 seconds** for
real cold import, versus the owner's approximately **135 seconds** in installed
preview 13. That historical comparison is roughly **6.8x lower latency**, but is
not a paired installed-build benchmark: builds/profiles differ, and preview QA's
endpoint was editor usability while this endpoint is all assets published. It is
not a D26 first-scene-frame or startup acceptance claim. Projects have fresh
content caches; OS and driver caches are not deliberately flushed.

### Adaptive admission failed its acceptance test

The machine was quiet, held for this bundle after batch 34. One GPU lease covered
eight Outpost cases, C: A/B/B/A followed by D: A/B/B/A. Control source is
`f9fc09c9`; candidate `a594a74c` combines parallel geometry with cross-process,
per-volume write leases and p90 latency feedback. Both include Color8/mips and
have identical package/feature sets, matching owned CLI/daemon/worker builds and
the same frozen player. All observations are retained:

- C: assets-ready seconds **20.099653 / 24.0846499 / 25.2615612 / 19.8313339**.
  Means **19.96549345 -> 24.67310555 s**, **23.6% slower**.
- D: assets-ready seconds **23.2907832 / 39.1448357 / 38.6021749 / 23.2392544**.
  Means **23.2650188 -> 38.8735053 s**, **67.1% slower**.
- Create-through-assets-ready means: C: **22.4131562 -> 26.1969949 s**;
  D: **25.0587296 -> 40.02238025 s**.

All **920 action keys**, **11,672 artifact references** and **7,274,884,128
physically verified referenced bytes** match v4; zero encoder fallbacks. Evidence,
catalogs, source IDs, feedback snapshots and verification logs were extracted
before each project was deleted. Exact target, ancestor and descendant reparse
checks passed. Peak retained usage was one current project, and all eight were
removed; no pending project-cleanup request remains. Canonical results:
`measurements/adaptive-cache-abba-2026-10-26.json`. Raw evidence:
`target/ara-dev/import-speed/adaptive-{c,d}-{a1,b1,b2,a2}-outpost/`.

The candidate is rejected. `bd5b5799` reverts it and restores the exact
`f9fc09c9` tree; parallel geometry remains **off**, with no write limiter or
per-volume fallback. The earlier plain-parallel NVMe pairing's 25.4-second
sequential baseline predates the texture improvements. Its 23.7% win cannot
establish a win against today's approximately 20-second control. A new quiet-slot
NVMe A/B/B/A must compare current sequential versus **plain parallel, no limiter**,
both with Color8/mips. Keep parallel off unless that pairing clearly wins; only
then reconsider a volume-specific policy. The observed regressions do not, by
themselves, identify which coordination or storage operation caused them.

## Plain parallel on the current baseline: 2026-10-26 — closed

A new quiet-slot NVMe A/B/B/A compared current sequential geometry with **plain
parallel, no limiter**, both with Color8/mips. The source experiment is `5428daff`;
the same frozen CLI/player and all package/feature sets are used for both workers.
No subsequent single-thread remap experiment enters these frozen binaries.

Assets-ready seconds A1/B1/B2/A2 are **21.5439145 / 19.4925563 / 19.5951161 /
19.4909498**. Means **20.51743215 -> 19.5438362 s (4.7% lower)**. This is not a
clear repeatable win: A2 matches B1 within **1.607 ms**, and the control's
**2.053-second** spread exceeds the **0.974-second** mean difference. There are
only two observations per variant; no statistical certainty is claimed.
Create-through-ready means are **24.2239774 / 22.48098105 s**.

All 460 action keys, 5,836 artifact references and 3,637,442,064 physically verified
referenced bytes match v4, with zero fallbacks. Each of the four projects was
reparse-checked and deleted after evidence extraction/verification. Results:
`measurements/plain-geometry-current-nvme-2026-10-26.json`; raw evidence:
`target/ara-dev/import-speed/plain-current-{a1,b1,b2,a2}-outpost/`.

**Decision: keep parallel geometry off.** `192b9d72` restores the flag to false.
Drop adaptive admission and the per-volume fallback; the older 23.7% NVMe gain
predated faster texture preparation. This scheduling investigation is closed.

## Byte-exact single-thread virtual-geometry remapping: 2026-10-26

Every virtual-geometry group previously built an ordered map from source vertex
ID to compact local ID. Replace that lookup with generation-tagged indexed
scratch reused within one cook. IDs still follow first encounter in the index
stream; no map iteration determined their order. Vertex bits, local indices and
source-ID arrays passed to every native meshopt call are identical. Sparse public
inputs retain the ordered map instead of allocating an oversized source lookup.
No extra threads, changed simplifier settings, importer/format revisions or keys.

A frozen baseline/candidate replay ran the 101 real Outpost model inputs with one
thread and verified complete output bytes against the production cache on **every
pass**, reusing verified textures without decoding/encoding/storing. One GPU lease
serialized the A/B/B/A phase probe while the integration gate was active. Summed
virtual-stage seconds: **6.0911706 / 3.7413036 / 4.1517075 / 5.4968285**.
Means **5.79399955 -> 3.94650555 s (1.47x, 31.9% lower)**. All observations and
per-model timings are retained in `measurements/geometry-remap-2026-10-26.json`.

This is a phase result under load, not a quiet fresh-import-wall claim. The
unchanged LOD-solve stage varies from **8.180 / 6.702 / 7.524 / 7.099 s**, so the
entire replay-time difference is not attributed to remapping. Both candidate
virtual timings are below both control timings. Raw source snapshots, hashes,+build logs and results: `target/ara-dev/import-speed/geo-efficiency/`.

Focused tests cover first-encounter IDs, every copied vertex bit including signed
zero, reuse across groups, generation wrap collisions and sparse allocation.
Existing deterministic DAG/edge/page tests remain enabled; the declared explicit
million-triangle benchmark remains opt-in.
