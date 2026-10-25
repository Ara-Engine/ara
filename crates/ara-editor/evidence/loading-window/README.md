# Loading window

Design: [`docs/design/editor-opening-and-assets.md`](../../../../docs/design/editor-opening-and-assets.md), section 1.
Open `gallery.html` to step through the states at 1× and 2×, Ara Dark beside Ara Light.

## Method

- A cold Outpost created with `ara hub create` and a fresh `ARA_CONFIG_DIR` (the Hub
  service is shut down right after), then `ara-editor <project> --verify-loading --dpi 1`
  and `--dpi 2`: the loading window runs offscreen on the real import pipeline (115
  sources) and nothing appears on screen. Dev profile (`CARGO_BUILD_JOBS=2`), Windows 11,
  RTX 4070 SUPER, on a shared machine.
- `run-*` (gallery: "live: …") are the real run's milestones: preparing, the first
  imported source, half way, opening. The real run had no failed imports.
- The other states render fixed statuses through the same window: preparing, importing,
  "Open now" with two failures, the review (three failures; more than four), opening
  early ("Open now") and opening.
- WebP at quality 90; PNG originals are in `target/tool-pages-png/loading-window`.
