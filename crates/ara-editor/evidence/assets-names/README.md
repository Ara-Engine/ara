# Assets panel and names: before and after

Design: [`docs/design/editor-opening-and-assets.md`](../../../../docs/design/editor-opening-and-assets.md), sections 2 and 3.
Open `gallery.html` to compare each state side by side.

## Method

- **Before:** `main` at `4b990d75`. **After:** round 4. Both through
  `--verify-panels --sample` from a fresh scratch project and isolated profile per size,
  at 1440×900 (1×) and 2560×1440 (2×), in Ara Dark and Ara Light, offscreen.
- The route's fixtures gained textures with real-world names
  (`dry_ground_01_diff_2k`, `rusty_corrugated_iron_nor_gl_2k`, `ContainerBlueVariantLarge`,
  `concrete_road_barrier_arm_2k`) and two captures: that folder as a grid
  (`assets-textures`) and as a list (`assets-list`). They exist after only.
- In the default layout Assets is a 240 px dock under the Hierarchy; the tiles and the
  list are the same at any width.
- WebP at quality 90; PNG originals are in `target/tool-pages-png/assets-names`.

## What changed

| Part | Before | After |
|---|---|---|
| Toolbar | Create, Import, search, Grid/List | Create, search, Grid/List (Import by drag and drop or the context menu) |
| Tiles | Names cut at the tile edge (`animation.anima`), different heights per row, a type bar and a type line | One 96 × 116 box per asset; the name without its extension centred on up to two lines, broken at `_`, `-`, `.` or camelCase, ellipsis after two; a type badge on thumbnails; columns centred in the dock |
| Placeholders | A white type icon | The type icon in its type colour |
| List | The full path (`textures/checker.png`) in one column | The type icon in its colour, the name without its extension (ellipsis at the column), its type; the folder while searching; the selected row highlighted |
| Hierarchy | `main.scene` | `main` (the path is the tooltip) |
