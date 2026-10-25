# First-hour QA fixes: before and after

The editor agent's first-hour QA (`--verify-first-hour`, findings sheet tiles E–H) found
three problems in the tool pages. Design notes: [`docs/design/editor-tool-pages.md`](../../../../docs/design/editor-tool-pages.md)
(sections 1, 2 and 7). Open `gallery.html` to compare each step side by side.

## Method

- **Repro:** `ara hub create <dir> --name "My Outpost" --template outpost` with a fresh
  `ARA_CONFIG_DIR` (the Hub service is shut down right after), then
  `ara-editor <project> --verify-first-hour` offscreen, cold: the route browses
  Assets > `assets/kit` while the 115 imports run, waits for them, opens the Material
  page and finishes with Problems and Console. This PC's Vulkan overlay layers (OBS,
  Overwolf) are not isolated, so their `ARA_GPU_ENVIRONMENT` messages appear as they did
  for QA.
- **Before:** the QA agent's run of the same route on `main` (`3aea98ca`).
  **After:** this branch. Dev profile (`CARGO_BUILD_JOBS=2`), Windows 11, RTX 4070 SUPER,
  on a shared machine with other agents building.
- The route captures Ara Dark at the size it is given. `before/1440` and `after/1440` hold
  the reviewed steps as WebP (quality 90); the PNGs are in `target/tool-pages-png/first-hour-qa`.
  Every step's capture, at 1440×900 (1×) and 2560×1440 (2×), is at
  `D:\AraBuild\tool-pages-evidence\qa-{before,after}`.

## What changed

| Finding | Before | After |
|---|---|---|
| Material page opens unreadable (E) | Fit-all zoom (titles about 5 px), seven empty preview tiles, the page preview still "Rendering…" when the step captured, texture names cut at 104 px | Opens at 0.85 or more with the output end in view; tiles only for previews rendered or on their way, a pending tile says "Rendering…"; the page preview is "Live" when the step captures; whole texture names on a caption line |
| Thumbnails of importing models (G, H) | Six `ARA4501 \\?\C:\…\apron.gltf has not been imported yet` warnings, one per 30 s, from "human"; thumbnails 30–90 s after their import | No reports: tiles keep their Importing badge and fill in as each import publishes (all twelve kit tiles by the time the imports finish). Real failures name project-relative paths, from the engine |
| Console (F) | ✕ 32 · ⚠ 38: this PC's `ARA_GPU_ENVIRONMENT` loader and overlay messages counted and listed as the project's, between the thumbnail warnings | ✕ 0 · ⚠ 0 in Console and Problems; one collapsed "This computer · 4" header holds them, with a note when opened |

Measured in the repro runs with temporary instrumentation (removed before commit): a cold
material preview cook took 0.3–0.7 s, and once 5.8 s while other agents were compiling
(the engine cook it replaces took about 1.2 s here, 3 s under load); a cached page preview
lands in under 10 ms. The first-hour report lists no Problems entries besides this PC's
GPU messages.

## Not changed here

- `container_blue.gltf` renders green because the sample kit multiplies the green
  `container_side` texture (mean sRGB about 93, 137, 81) by `[0.42, 0.55, 0.68]`
  (`crates/ara-samples/src/build/outpost.rs`); a multiply cannot turn green into blue.
  The thumbnail is faithful; the fix belongs to the sample kit.
