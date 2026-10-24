# Tool pages: before and after

Design: [`docs/design/editor-tool-pages.md`](../../../../docs/design/editor-tool-pages.md).
Open `gallery.html` to compare each page side by side.

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

## Known issues found (not from this change)

- `tests/native.rs` golden `ara-dark` fails on the base commit with the identical score
  (1-SSIM 0.00866): the golden predates the current title bar (problems chip).
- `tests/ux.rs::inactive_list_tabs_stay_mounted_hidden_and_come_back_as_they_were` is
  flaky under the full suite on the base commit too (2 of 8 runs).
