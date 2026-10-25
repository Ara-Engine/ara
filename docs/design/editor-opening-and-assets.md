# Opening a project, the Assets panel and names

Owner feedback from preview 8: a new project opened at once and its models popped in
over minutes; Assets tiles had names aligned to the right and uneven rows; names
showed file extensions everywhere. This round makes the editor open on imported
content, rebuilds the Assets tiles and list, and drops extensions from names.

References: Unity's Project window (`D:\AraRefs\editor-ui\unity\project-1.png`,
`project-2.png`), UE's Content Browser (`unreal\content-browser-1.png`), Unity's and
UE's startup splash (an asset import progress window before the editor appears).

## 1. The loading window

**Benchmarks.** Unity shows a small window while a project imports ("Importing
(iteration 2)", the current asset, a bar) and opens the editor when it is done. UE
shows its splash with the stage and percentage. Neither lets you work early; neither
shows failures until the editor is up.

**Where Ara beats them.** The window lists failed imports inline with their errors,
offers "Open now" after a few seconds for people who want to work while importing, and
is skipped entirely when the imports are already done.

**Behaviour** (`crates/ara-editor/src/loading/`):
- The editor process opens the project and starts its authority and EP server, then,
  before creating the main window, shows a 520 px wide undecorated window, centred on
  the pointer's display, rounded on Windows 11 with the theme's hairline border, and
  draggable anywhere but its button. The authority ticks while it is up, so imports
  progress and agents can connect.
- It is driven by `ara_play_host::ImportProgress` (done, total, pending, failed):
  - **Preparing assets** until the pipeline first reports;
  - **Importing assets · N of M**, a bar, "Imported <path>" for the source that
    finished last, and the elapsed time;
  - failures appear as rows (warning icon, path, the error's first line; four at
    most, then "N more in Problems"), and the window grows to show them;
  - when every import has settled with failures, **Imported N of M · K failed** waits
    for **Open project** (Enter), as Unity's startup waits on compile errors;
  - when every import has settled cleanly, **Opening <project>**, and the editor opens.
- **Open now** appears after 3 s; the editor then opens at once and imports carry on
  there (the Importing chip, Assets badges and the Scene notice still show them).
- A project whose imports settle within 0.4 s (a warm cache, no sources) opens with no
  window at all. A pipeline that never reports (no import worker) opens after 12 s.
  If the window cannot be created, the editor opens as before.
- The main window is created hidden behind the loading window and shown when it has
  presented its first frame, so the hand-over never shows a black window. Closing the
  loading window (Alt+F4) quits without opening.
- **Look:** the Hub's language: the Ara wordmark and a version tag, the project name
  (20 px) and folder (middle-elided), a 14 px stage title with its count, a 6 px
  rounded bar in `primary` (`warning` on review), muted captions, the elapsed time in
  JetBrains Mono, Hub buttons (34 px, `primary` for Open project).
- **Hook:** `app::run` calls `loading::run` and hands the splash to the main window
  after its first present: three small additions. Bounded runs skip it.

## 2. Assets panel

**Benchmarks.** Unity: 64 px thumbnails on a regular grid, the name centred under each
(no extension), a size slider that turns the grid into a list. UE: fixed-size cards,
the thumbnail on a dark square, a coloured type strip, the name wrapping to two lines
and the type under it. Both keep every tile the same size, so rows align.

**Changes** (`shell/assets_panel.rs`, `shell/names.rs`):
- **No Import button.** Files come in by drag and drop or the context menu's Import
  (and the command palette). The empty folder state says so.
- **Tiles:** one 96 × 116 box per asset, 8 px apart, so every row and column aligns;
  a 64 px thumbnail centred at the top; the name without its extension centred under
  it on at most two lines, breaking at `_`, `-`, `.`, spaces or camelCase humps, with
  an ellipsis when two lines are not enough (fitted from one shaping pass and cached).
  Assets without a thumbnail show their type icon in the type colour on a quiet square;
  thumbnails carry a small type badge on their bottom corner instead. The import
  badge (importing, failed) sits on the top corner. The tooltip has the file name with
  its extension, the type and the import state.
- **Short drawers** (too short for a tile) use 184 × 64 tiles: a 48 px thumbnail with
  the name beside it, left-aligned, on at most two lines.
- **List:** two aligned columns: the type icon (in the type colour) and the name
  without its extension, then the type in words ("Model", "Texture", …) or, while
  searching, the folder. The selected asset's row is highlighted.

## 3. Names without extensions

Names show what a thing is called; its type shows as an icon and badge, and the
extension lives in the tooltip and the Inspector:
- Assets tiles and list rows: `apron`, not `apron.gltf`.
- The Hierarchy's scene root: `main`, not `main.scene` (its path is the tooltip).
- Entities placed from a model were already named by its file stem.

## Verification

- `ara-editor <project> --verify-loading --dpi 1|2` runs the loading window offscreen
  on a cold, Hub-created Outpost: its real milestones (preparing, importing, half way,
  review or opening) and every state from fixed statuses (preparing, importing, open
  now with failures, review, review with more than four failures, opening), in both
  themes.
- `--verify-panels` captures Assets (models, samples, a textures folder with long
  real-world names, and the same folder as a list) and the Hierarchy at 1440 × 900 and
  2560 × 1440, in both themes.
- Tests: `names` (stems, two-line fitting, separators, ellipsis), `loading::view`
  (stages, counts, failures, markup parses).
