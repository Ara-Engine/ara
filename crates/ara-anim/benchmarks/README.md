# Reproducing the animation performance pass

Run these commands from `crates/ara-anim` on Windows. The source clone, build
trees, source asset export and executables stay under gitignored `perf-local/`.
The committed JSONL files contain individual timings and background load.
Pinned revisions, settings and executable hashes are in
[environment.json](environment.json); both clip inventories are in
[compression.json](compression.json).

```powershell
$env:CARGO_BUILD_JOBS = '2'
cargo build -p ara-anim --release --example crowd --example stages --example compression
New-Item -ItemType Directory -Force perf-local | Out-Null
Copy-Item ../../target/release/examples/stages.exe perf-local/candidate-stages.exe
Copy-Item ../../target/release/examples/crowd.exe perf-local/candidate-crowd.exe
python benchmarks/build_baseline.py

git clone --depth 1 https://github.com/guillaumeblanc/ozz-animation.git perf-local/ozz
git -C perf-local/ozz fetch --depth 1 origin 744eb9d99f606eda849acb0b1204f7a3dc20bca1
git -C perf-local/ozz checkout --detach FETCH_HEAD
$cmake = 'C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe'
& $cmake -S benchmarks -B perf-local/ozz-build -G 'Visual Studio 18 2026' -A x64 -DCMAKE_GENERATOR_INSTANCE='C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools'
& $cmake --build perf-local/ozz-build --config Release --target ozz_bench --parallel 2

perf-local/candidate-stages.exe export perf-local/assets.json
python benchmarks/run.py --frames 300 --rounds 3 --output perf-local/stages.jsonl
python benchmarks/run.py --frames 300 --rounds 3 --crowd --workers 1 2 8 16 --output perf-local/crowd.jsonl
python benchmarks/run.py --frames 300 --rounds 3 --round-offset 3 --crowd --workers 16 --output perf-local/crowd.jsonl
python benchmarks/chunks.py
../../target/release/examples/compression.exe
```

The baseline script checks out only the frozen animation source into an ignored
standalone workspace, links the unchanged dependency crates, and copies the same
current timing harness. Its sole harness adaptation removes the new offline
`runtime_bytes` print. The engine checkout and its root manifest are never edited.
Baseline: `4e01c27501da1f25f28f2d26db81371511068dad`.

All executables run below normal priority. Timers cover 300 frames after 30 warmup
frames. Pools, asset cooking/loading, source accuracy checks and actor creation
are outside the timers. Each actor has its own caches and buffers. Neither
library shares poses, even when both lanes have matching normalized phases.
The frozen and candidate Ara builds use the same Rust toolchain and default
release settings. Ozz uses MSVC release, its default SSE2 implementation and no
AnimationOptimizer or key removal. The exported floats, skeleton graph, two clips,
four samples, two lane blends and final cross-fade are the same.

`stages` measures skeletal kernels: sample four clip instances, blend three pairs,
local-to-model, palette, or all four stages in one actor task. Curves, notifies,
root extraction, controller transitions and snapshot copies are excluded from
these kernel comparisons in both implementations. The separate `crowd` workload
retains all of those responsibilities, including sync groups. Ozz's skeleton
builder reorders joints depth-first; the adapter remaps the original IDs and
clips to that order. Ozz pads 65 joints to 68 SIMD lanes. Both palettes use the
same bind transforms and derived inverse binds. Ozz's measured errors are against
the imported source values at all keys and 16 subdivisions, outside the timer.

`run.py` records Windows system CPU time minus the child benchmark's CPU time,
normalized across all 16 logical processors. This is other-process CPU load,
averaged over each stage process, or between successive crowd result rows; those
windows include setup/warmup. It is not an instantaneous load at the p50 frame.
The first row's window also includes cooking. Each reported timing has its load
next to it in the main README and an exact per-run reading in the raw JSONL.
Runs alternate A/B/C and C/B/A order; crowd passes alternate A/B and B/A. A table
cell is the median of run p50s and the median of run p95s, not a pooled p95.
Isolated stage medians need not sum to the combined-kernel median.

Record fresh results to new files: the runner appends. Run benchmark drivers
serially, without compiling or running another benchmark alongside them.
Exploratory and earlier-format runs are retained locally under `perf-local/` and
excluded from the committed final measurements. Saturated-host trials were
repeated; all final repetitions remain in the committed data, including tails.

The source export SHA-256 is
`cda4c86b7cfde29be4f682fbf1691a5e03628213c5dc9aa47f5d6d5a44d57ecf`.
The JSON contains scalar curves/notifies as well; the skeletal comparison imports
only its skeleton and joint channels. Source assets are generated, not downloaded.
The ozz clone retains its upstream MIT license. No upstream implementation is
copied into the production crate or this adapter.
