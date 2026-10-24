# ara-ui-test

Purpose: native `capture.ui`, structured inspection, semantic tests and perceptual PNG comparisons.
Library API only; protocol/MCP and UI Designer wiring are host integration work.

Public API:
- `Harness::{mount,query,event,activate,advance,capture,run}` uses the actual retained UI runtime.
- `Harness::expect_*` checks focus, selector states, visible text, control/bound values and actions.
- `CaptureEngine::{document,capture,image}` renders the production display list through CpuGpu.
- `Capture::{write,compare}` writes PNG/report JSON and readable golden failure artifacts.
- `report_schema` returns the generated JSON Schema; `register` registers report types.
- `compare` uses local SSIM, perceptual channel error and a changed-pixel budget, including alpha.

Tests: `CARGO_BUILD_JOBS=2 cargo test -p ara-ui-test`. `tests/responsiveness.rs` drives a
reconciling host through `InputState`: first-click tabs/menus/notification dismissal across
rebuilds, batched (slow-frame) and moved-under-the-pointer clicks, echoed field edits, zero-work
idle frames and a theme switch bounded to one restyle with no layout or shaping.
Run `cargo clippy -p ara-ui-test --all-targets -- -D warnings` and package rustfmt check.
`ARA_UI_UPDATE_STORY_GOLDENS=1` explicitly rebaselines committed catalogue goldens.
Inspect all changed captures before accepting a rebaseline. Never auto-accept a missing golden.

Gotchas:
- Logical dimensions use a 96-DPI baseline. Raster glyphs are generated at physical DPI.
- Bundled fonts are pinned Latin + Arabic; hosts supply licensed additional font resources.
- RTL is a layout-box mirror pass. Glyphs, model data, action names and logical tab order stay intact.
- Capture startup primes the preferred font with Latin text so secondary handles identify fallback use.
- Transparent surfaces, overlapping imagery and uncertain coverage never receive invented contrast ratios.
- Fixture models are immutable snapshots. Bound-value assertions read native controls; hosts consume actions.
- Input events traverse InputState, including scopes, capture and IME. No direct widget activation shortcuts.
- Story animation tracks are isolated preview state, sampled from an explicit integer clock.
- No window, sleeps, wall clock, OS clipboard or GPU device is required.
