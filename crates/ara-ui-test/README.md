# Native capture and semantic UI tests

This is the library implementation of `capture.ui`, with no window, GPU device, network,
wall clock or OS clipboard. `CaptureEngine::document` captures arbitrary native source;
`Harness::mount` captures/tests a schema-registered story. Both use Ara's production
Taffy/Parley/display-list pipeline and `ara-ui-render::CpuGpu`, including its real batches.
Raster glyphs are generated at physical DPI, rather than stretching a low-DPI screenshot.

```rust,no_run
use ara_schema::SchemaDb;
use ara_ui_stories::{StoryBook, Query};
use ara_ui_test::Harness;
# fn main() -> ara_ui_test::Result<()> {
let mut db = SchemaDb::new();
ara_ui_stories::register(&mut db)?;
let book = StoryBook::read(include_str!("examples/catalogue.stories"), &db)?;
let (_, story) = book.stories()?.into_iter().find(|(_, s)| s.name == "pause").ok_or("pause")?;
let mut test = Harness::mount(&story, &story.views[2], &db)?; // Arabic, gamepad, dark, 144 DPI
let resume = Query::role("button", Some("loc:resume"));
test.activate(&resume)?;
test.expect_focused(&resume)?;
test.expect_action("game.resume", None)?;
test.advance(100)?;
test.capture()?.write(std::path::Path::new("captures"), "pause-ar")?;
# Ok(()) }
```

Queries require exactly one visible match; `query_all` returns all matches in reading order.
`Query::label`, `Query::text` and `Query::test_id` supplement role/name queries. Explicit
`test-id` values survive component expansion; absent test IDs fall back to stable node IDs.
Hidden/clipped controls do not match, disabled controls are observable but cannot activate.
Role queries share the production AccessKit role resolver (`textbox`, `checkbox`, `combobox`,
`progressbar`, `button`, `dialog`, `tab`, `tablist`, `tree`, `list`, `table`, `separator`,
`img`, `text`, `tooltip`, `scrollview`, `menu`, `generic`).

Use `event(InputEvent)` for pointer press/move/release, wheel, keyboard, controller and
`EditEvent::{ImeStart,ImePreedit,ImeCommit,ImeCancel}`. `key`, `gamepad`, `focus`,
`push_scope` and `pop_scope` wrap native input routing. IME preedit is visual-only until
commit. `expect_value`, `expect_text`, `expect_state`, `expect_bound` and `expect_action`
observe the actual retained runtime and queued named game commands. The fixture remains
an immutable model snapshot: bound assertions inspect controls; the host validates and
dispatches action payloads at the simulation boundary. No test executes gameplay commands.

The UI clock is an explicit `u32` millisecond count. `advance` samples transitions at the
new absolute time without sleeping; single-step and subdivided stepping produce identical
captures. These are story-owned paint-translation tracks; hosts can advance additional
game/editor animation systems from the same explicit clock. RTL reflects layout boxes,
clips and chrome, preserving paragraph glyph direction, stable IDs and logical tab order.
Remount when changing locale direction; do not toggle mirroring on an already mounted UI.

## Report contract

[`schema/capture-ui.schema.json`](schema/capture-ui.schema.json) is the generated,
self-contained JSON Schema 2020-12 contract. `register` installs `CaptureReport` and its
dependencies into any host Schema DB; `report_schema()` generates the same schema.
`Capture::write` returns `[png_path, report_path]`; report text uses Ara's canonical writer.
Defaults may be omitted in canonical JSON; readers materialize reflected zero defaults.
Version 1 fields are:

| Field | Meaning |
|---|---|
| `version`, `view`, `pixels` | Contract version, full capture configuration, physical PNG dimensions. |
| `story`, `fixtureType`, `focused` | Story/model provenance and stable focused identity, when available. |
| `nodes[]` | ID/tag/parent, actual accessible role and localized label, visibility, display text, committed value, logical bounds/clip/text bounds. |
| `nodes[].style` | Resolved foreground/background/border RGBA, font size, radius, border, display, flex direction, clipping and T/R/B/L padding. |
| `nodes[].provenance[]` | Production cascade records: full `selector`, `property`, `authored`, `resolved`, rule `byte` and declaration `valueBytes` range; inline overrides use `local`. Offsets address the combined editor CSS + story stylesheet. |
| `nodes[].shaping` | Production Parley evidence: text, line/glyph counts, missing glyphs, RTL/LTR run counts, UTF-8 cluster ranges and glyph IDs. |
| `focusGraph[]` | Stable-ID vertices with tab/left/right/up/down neighbors. Harness reports honor the active screen/modal scope. |
| `batches[]` | Actual physical renderer pipeline, texture, clip index, instance start/count and break cause. |
| `diagnostics[]` | Stable code/node, message, suggested source fix, optional measured/threshold values and uncertainty. |
| `work`, `commands`, `glyphs` | Native work counters and display-list costs for this capture update. |

Diagnostic codes: `ARA7100` parent/viewport overflow, `ARA7101` text/content/clip overflow,
`ARA7102` missing glyphs, `ARA7103` explicit fallback use, `ARA7104` measured WCAG
contrast (passes and failures), `ARA7105` uncertain contrast, `ARA7106` touch targets.
Solid contrast uses sRGB linearization and alpha composition, with a 4.5:1 threshold
(3:1 at 24 logical px and above). Imagery and unbacked transparency yield uncertainty
with no invented ratio. Touch checks use the clipped logical size and a 44px minimum.
Fallback reporting treats the Latin-primed preferred face as handle zero. Supply the
needed licensed font packs through `CaptureEngine::with_fonts`; bundled fonts cover
Latin and Arabic. Suggested overflow fixes preserve accessibility text size.

## Golden suite and artifacts

Set `CARGO_BUILD_JOBS=2`, then run:

```powershell
cargo test -p ara-ui-stories -p ara-ui-test -p ara-ui -p ara-ui-document
cargo clippy -p ara-ui-stories -p ara-ui-test -p ara-ui -p ara-ui-document --all-targets -- -D warnings
cargo fmt --check -p ara-ui-stories -p ara-ui-test -p ara-ui -p ara-ui-document
cargo run -p ara-ui-test --example capture -- pause 2
```

The complete suite runs 30 stories / 180 fixed views, compares committed PNGs before and
after interactive recipes (222 snapshots), and executes every semantic recipe. It also
checks the persisted canonical catalogue. `evidence/` is
ignored generated output. Ordinary tests never create missing baselines. To intentionally
rebaseline, set `ARA_UI_UPDATE_STORY_GOLDENS=1` for the suite command, inspect every changed
PNG, then unset it. `python examples/review_gallery.py` makes labelled contact sheets
from the actual evidence PNGs (optional Pillow helper). `export_contracts` regenerates
reviewed schemas and the minimal story example through canonical writers.

The comparator applies local 8x8 SSIM loss, mean weighted color/alpha error and a changed
pixel fraction budget. Default limits are 0.01 / 0.003 / 0.005 with a 0.02 per-pixel
threshold. All budgets and dimensions must pass. This is a deterministic CPU metric;
it does not claim GPU/vendor equivalence or implement FLIP. `Capture::compare` writes
actual/expected/amplified magenta diff PNGs plus a mini-tree failure file on mismatch.
Semantic failures include expected/actual values and the native tree.

## Integration seams

The Engine Protocol `ara_capture` adapter should select `capture.ui`, call
`CaptureEngine::document` or `Harness::capture`, store PNG/report bytes in its resource
store, and return resource handles plus the report. `ara_test` loads a `StoryBook`,
mounts each view, runs `Harness::run` and compares selected baselines. Register story and
report schemas in the shared schema database so catalog/docs generation stays unified.
No protocol types or MCP server are linked here while `ara-proto` is in flight.

The native UI Designer uses the same mounts and capture reports for canvas previews,
style provenance, overflow/focus overlays and fixture/device/locale/theme/time controls.
Designer source changes continue through `ara-ui-document::Command -> SourceOp`, while
story edits use `StoryBook::edit -> ara_doc::Op`; the host owns attribution, revisions,
transactions and D20 scheduling. Run capture work in a background worker/process and
publish completed results at a frame boundary. CPU snapshots are offline authoring work,
not a renderer inserted into the human's frame loop. `ara-gpu` integration can later
replace the adapter while preserving story, semantic and report contracts.
