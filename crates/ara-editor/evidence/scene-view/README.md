# Scene view: before and after

Design: [`docs/design/editor-scene-view.md`](../../../../docs/design/editor-scene-view.md).
Open `gallery.html` to compare each state side by side.

## Method

- **Before:** `main` at `75a66c7e`, plus only the panels-route change that captures the
  gizmo states (hovered axis, dragged plane, rotate, scale) and the viewport menus.
  **After:** round 3.
- Dev profile (`CARGO_BUILD_JOBS=2`), Windows 11, RTX 4070 SUPER, offscreen
  `--verify-panels --sample` from a fresh scratch project and isolated profile per size,
  at 1440×900 (1×) and 2560×1440 (2×), in Ara Dark and Ara Light.
- `before/1440` and `after/1440` hold the reviewed states as WebP (quality 90, legibility
  checked); the PNGs are in `target/tool-pages-png/r3-scene`. Full sets for both sizes are at
  `D:\AraBuild\tool-pages-evidence\r3-scene-{before,after}`.

## What changed

| Part | Before | After |
|---|---|---|
| Toolbar | Overflowed at 1440 ("Show" cut, Frame hidden); text on tools, space and pivot; three bordered snap splits | Icon tools, space and pivot as state icons, one magnet plus three snap values, icon Show; words drop on narrow bars (2560@2× shows the narrow form) |
| Move gizmo | Lollipop arrowheads, 4 px plane dots, a white centre dot | Solid 12 px cones, plane squares in their world planes (20 % fill, 1 px edge), a 10 px centre |
| Hover and drag | Hovered arm yellow; dragged handle's constraint line | The same, plus a dragged plane fills yellow and draws both constraint axes |
| Rotate | Three rings at full strength, tangled | The half of each ring facing away draws at 25 % |
| Scale | Dot ends | Filled 8 px cubes and a uniform-scale centre square |
| Axis widget | Positive caps showed "−", negatives "+", letters invisible on the discs; labels read "XX view" | X, Y and Z on the positive caps, unlettered negative rings, "+X view" |

## Gates

`cargo clippy -p ara-editor -p ara-editor-gpu --all-targets -D warnings`, `cargo test -p
ara-editor -p ara-editor-gpu` (with the new gizmo hit-target and axis-letter tests) and the
native goldens, rebaselined with `ARA_EDITOR_UPDATE_GOLDENS=1`: `ara-dark`, `ara-light`,
`palette-dark`, `palette-light` (toolbar, axis widget and gizmo). `main`'s `ara-dark` golden
already failed before this change (1-SSIM 0.0080): its title bar now shows a ⚠ 1 problems
chip from the verify route's overwrite notices. The rebaseline includes that chip.
