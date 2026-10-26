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

The controlled fresh-project paired wall matrix is deliberately pending the
owner's separate quiet slot. Neither the diagnostic reduction nor the retained
55-second storage outlier is a claim of Showcase below 10 s or Outpost below 20 s.
Canonical evidence is in `measurements/parallel-geometry-2026-10-26.json`; raw
traces, per-model outputs, frozen worker hashes and logs remain under
`target/ara-dev/import-speed/` (`cold-mesh-profile-a0-outpost/`, `geometry-*`,
`cache-close-a0/`, `mesh-final-*`, `mesh-gpu-protocol/`).
