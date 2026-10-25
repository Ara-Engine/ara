# Tool pages: before and after

Design: [`docs/design/editor-tool-pages.md`](../../../../docs/design/editor-tool-pages.md).
Open `gallery.html` to compare each page side by side (round 1 and round 2).

## Method

- **Before:** commit `497e8dec` (the preview 4 source, same as `D:\AraBuild\preview-src`) plus only
  the panel-route fix below. **After:** this branch.
- Dev profile (`CARGO_BUILD_JOBS=2`), Windows 11, RTX 4070 SUPER, offscreen
  `--verify-panels --sample` from a fresh scratch project and isolated profile per size,
  at 1440×900 (1×) and 2560×1440 (2×), in Ara Dark and Ara Light. Implicit Vulkan layers
  were disabled. `ara-import-worker` sat beside the editor, as in a packaged build: the
  play host publishes models through it (without it, no model is ever published and the
  old thumbnail silently rendered a cached placeholder box).
- `before/1440` and `after/1440` hold the reviewed pages as WebP (quality 90, legibility
  checked); the original PNGs are in `target/tool-pages-png` and on D: below. The full matrix (both sizes,
  every route page, layout reports and timings) is kept out of git to save space, at
  `D:\AraBuild\tool-pages-evidence\{before,after}\{1440,2560}` on the Windows machine.
- `thumbnail-studio.webp`: studio shots of three sample models, a clay capsule and three
  material spheres, from `cargo test -p ara-editor --lib studio -- --ignored` (writes
  `target/thumbnail-studio`).

## Panel route fix (both builds)

Since folder navigation (UX v2) the Assets root lists only folders, so phase 3 waited
forever for thumbnails it never requested, and its filename check could not pass. The
route now captures the `models` folder (`assets-*`: mesh tile and its import preview),
then `samples` (`assets-samples-*`: material and VFX tiles), captures the unfiltered Add
node popup (`material-add-*`), and switches animation views through the toolbar.

## What changed, per page

| Page | Before | After |
|---|---|---|
| Thumbnails | White, blown-out model silhouette on black; 384 px drawn into 64 px tiles (6:1 aliasing); spheres at 63 % with jagged edges; metals near black | Real materials, fitted 3/4 studio shot on one neutral backdrop; spheres at 88 % with softbox reflections; 512 masters resampled to 2× each view |
| Material | Registry ids on nodes (`material.parameter`), `out`/`in0` pins, 9 px subtitles, permanent minimap, `New` in every toolbar | Human titles, category-tinted headers, inline values, typed hollow/filled pins with argument names, dimmed unused outputs, breadcrumb toolbar, preview filling the sidebar, typed node Inspector |
| Add node | Raw `material.abs`… list, half the popup empty | Grouped by category, human names, signatures, doc tooltips |
| VFX | Fixed far camera (effect small), broken per-row rails, values cut mid-way, name and kind run together | Preview framed on simulated particles, unbroken stage rails, name + muted kind, summaries that fit |
| Animation | Transitions as an X between `in`/`out` stubs, a list strip with raw ticks, text fields for enums | State machine with Entry marker, straight arrows with condition labels, Parameters + Transitions sidebar, status strip, typed Inspector with blend plot |
| Import | Labels stacked above fields, raw `{}` | Header with preview, 24 px property rows, segments for enums and quality |

## 500-node canvas benchmark (dev build, route's 1,000 warm samples)

| Run | Size | Input-to-present p95 | Paint p95 |
|---|---|---:|---:|
| Before | 1440×900 | 9.494 ms | 0.962 ms |
| After | 1440×900 | 4.269 ms | 0.789 ms |
| Before | 2560×1440 | 6.758 ms | 0.795 ms |
| After | 2560×1440 | 5.073 ms | 0.606 ms |

Canvas paint cost did not regress; the totals vary with the shared machine's load.

## Round 2: node previews, module enable, Play, and the remaining tool panels

Same method and machine. `r2/before/1440` is `7c728927` plus `dda5b934`: without that
fix the Agents panel could not be built once an agent was connected (its Follow
tooltip broke the markup), so there was nothing to capture. `r2/after/1440` is
`abe931af`. In-node previews and the VFX checkbox are compared with round 1's after
shots. The panels route now seeds the four panels: diagnostics on the bus, an agent
presence and three agent turns, and Settings docked in the document area. Full PNG sets
for both sizes are at `D:\AraBuild\tool-pages-evidence\r2-{before,after}`, and the
2-{before,after}`, and the
reviewed PNGs are in `target/tool-pages-png/r2`. The 500-node benchmark was not
re-measured: these builds overlapped other compiles on the shared machine.

| Page | Before | After |
|---|---|---|
| Material | No node previews | 144 px value previews on every operation node (sphere when the value depends on the surface, swatch otherwise); text-authored graphs laid out left to right |
| VFX | A module could only be removed | An enable checkbox per module; a disabled one stays in place, dimmed, skipped by simulation, codegen and rendering |
| Animation (Play) | Nothing live | The playing state rings green with its cycle; fades fill; parameters read live (not capturable offscreen: needs Play) |
| Profiler | Cards: a mostly empty flame graph, GPU passes and counters as cards, frame bars without a budget line | Frame strip with a labelled budget line, ruler, thread lanes plus GPU queue lanes, Scopes table (calls, total, self, share), Frame counters column, dense Compare with diverging bars |
| Console | In the 240 px dock: time, message and source drawn over each other | Two-line rows: message and count, then time, origin and a left-aligned source link |
| Problems | Go and Why buttons over the message; text cut after a few words | Message at full width; code, source, Go and Why on the second line; fixes on their own line |
| Settings | A third line per setting for its key; reset at the far edge | One description line (key in the tooltip); reset beside its control; shortcuts grouped by menu |
| Agents | Reasons cut on one line; "r22"; "~1 +1" | Reasons on two lines; "Claude · just now · 1 edit" |

## Known issues found (not from this change)

- `tests/native.rs` golden `ara-dark` failed on round 1's base commit (it predated the
  title bar's problems chip). It passes at `7c728927`.
- `tests/ux.rs::inactive_list_tabs_stay_mounted_hidden_and_come_back_as_they_were` was
  flaky on round 1's base (2 of 8 runs). It passed in every full run this round (6 of 6).
- `ara_noise` in `ara-material/shaders/engine.slang` is discontinuous across lattice
  cells: its sine hash, multiplied by 43758.5453, amplifies rounding differences.
  Visible in the noise node's preview.
