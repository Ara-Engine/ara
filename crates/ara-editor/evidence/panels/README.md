# Editor panels v1 verification

Source: `9d8e20ea50c30ebf752ef99ed25cfacb4ca0fb2c`, Windows 11,
RTX 4070 SUPER, release profile, `CARGO_BUILD_JOBS=2`, Vulkan validation enabled.
Main was merged through `2d20bd8` before the final source gates. No importer
sidecar-version policy was changed. The approved additive source-path APIs and
their focused tests/docs are in commit `125fbe6`.

## Measurements

**Measured on a shared, loaded host.** This machine has 16 logical CPUs. Other
agents' builds and the parent's workspace tests were not paused. The parent
requested completion without an exclusive window and will rerun performance
benchmarks together on a quiet machine after integration. The final matrix had
already started when that instruction arrived; it completed without another wait
or retry. Its start observation was 21% total CPU and no compiler processes;
that observation does not establish isolation for either run.

| Run | Physical size / DPI | Pan/zoom p50 | p95 | p99 | Maximum |
|---|---|---:|---:|---:|---:|
| Earlier pass, before compact drawer fix | 1440x900 / 1 | 1.8008 | **3.1002** | 4.6754 | 6.3554 |
| Earlier pass, before compact drawer fix | 2560x1440 / 2 | 2.1966 | **3.5037** | 4.2891 | 5.5544 |
| Compact drawer, first run | 1440x900 / 1 | 1.6260 | **4.0371** | 4.3538 | 15.9381 |
| Reflow fix, contended run | 1440x900 / 1 | 2.9243 | **11.2370** | 17.1973 | 22.5733 |
| Final matrix | 1440x900 / 1 | 1.8368 | **3.1629** | 4.4764 | 5.2296 |
| Final matrix | 2560x1440 / 2 | 2.1074 | **2.8820** | 4.0733 | 4.8353 |

Times are milliseconds, with 1,000 warm samples per row and a 500-node material
graph open. Every sample includes elapsed input handling, painting, UI compile /
upload / submit, serialized native presentation and accessibility work. Individual
stage percentiles cannot be added to obtain the total percentile. No outliers
were removed and no scheduling/presentation time was subtracted.

The final idle CPU p95 was 0.1257 / 0.1333 ms (1,500 samples per size). Idle frame
interval p95 was 6.6801 / 6.7015 ms at the 144 Hz target. Interval maxima of
114.4307 / 152.6368 ms remain in the raw samples; the loaded host does not support
a claim of hitch-free scheduling. Cold startup/pipeline work and the single cold
interaction sample in `timings.json` are separate from the warm graph benchmark.

`1440/` and `2560/` contain final images, layout JSON, `panel-interactions.json`
(all raw graph samples plus five per-frame stages), and `timings.json` (idle and
render samples). `measurements-before-final/` preserves earlier passes, misses,
the prior batching experiment and an explicitly invalid window-extent run.
`load-observations.json` records available host samples and their temporal limits;
it does not invent CPU readings for runs that lacked a sampled observation.

## Functional and native gates

Both final release routes passed without weakening the p95 <4 ms gate. Each
completed real mesh import, thumbnail drag into the scene through typed ops,
undo/redo, import-settings edit and isolated reimport, UI node search/create,
typed socket connection, keyboard undo, and freezing/comparing actual CPU/GPU
profile captures. Both routes reached clean Vulkan validation before writing
`timings.json`. `ARA_GPU_ENVIRONMENT` loader messages do not count as validation
errors, following ara-gpu's environment classification.

Local gates passed:

- `cargo test -p ara-editor -p ara-host -p ara-doc -p ara-ops -p ara-ui -p ara-ui-render -p ara-ui-document -p ara-ui-test`
- `cargo clippy -p ara-editor -p ara-host -p ara-doc -p ara-ops -p ara-ui -p ara-ui-render -p ara-ui-document -p ara-ui-test --all-targets -- -D warnings`
- After the final grid reflow: `cargo test -p ara-editor --tests` and strict
  `cargo clippy -p ara-editor --all-targets -- -D warnings`.
- The explicit ignored native interaction test passed: four reviewed Scene /
  palette goldens each had `1-SSIM=0`, with GPU/CPU raster, MSDF, scalable glyph
  and analytic-line comparisons passing and validation clean.
- `cargo fmt --all -- --check`.
- `verify-panels.ps1 -ScratchRoot D:/AraBuild/editor-panels-verified-v6` completed
  the release matrix at both exact physical extents, using vendored SDL3 and the
  process-local `VK_IMPLICIT_LAYER_PATH` isolation.

## Visual review

All 36 final PNGs were inspected: Assets/import settings, Material/DSL/preview,
VFX stack/preview, animation states and blend spaces, Scene, Profiler, capture
comparison, and the 500-node graph, each in dark/light at 1440x900 and 2560x1440.
All files' physical extents were checked. Full-resolution HiDPI asset review
confirmed that compact horizontal tiles keep their captions inside the drawer.

I would accept the result as v1. Controls, borders, typography and selection
states are consistent; the panels are calm and legible; previews contain actual
engine output. Scrolling remains necessary in the short HiDPI inspector and
profiler view. The large graph capture intentionally shows a panned/zoomed region
of the 500-node canvas. The sphere-and-axis-cap gizmo supports canonical view
snaps. The previous hex clipping/right-edge spacing issues are corrected.

Open [`gallery.html`](gallery.html) to compare themes and sizes for every panel.
