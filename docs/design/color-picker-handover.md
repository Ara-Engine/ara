# Colour picker: handover notes (no code yet)

Branch `t3/ara-color-picker` merges inspector handover `4b953ca6` (popover primitive `dd5a617c`
included). No picker code was written; these are the findings and the design.

## Seams found
- Placeholder: `crates/ara-editor/src/shell/inspector_popup.rs` (`Shell::color_picker: Option<String>`,
  `Binding::ColorPicker(path)`, anchors `swatch-…`/`nest-swatch-…`, `inspector_popovers`,
  `inspector_popup_contains`, `dismiss_inspector_colour`). Mounted from `shell/mod.rs`
  `rebuild_main`/`rebuild_float`; routed in `app/controller.rs` (~L462, L515) and `shell/input.rs` L39;
  Escape in `shell/keyboard.rs` (~L156).
- Gesture pattern to copy: `shell/scrub.rs` (preview via a `@scrub|path` draft marker so SceneSync
  re-previews; `draft_records` in `input.rs` projects it; release commits one `editor.apply`;
  `editor.field_gesture`/`end_field_gestures` bracket it). The picker needs an `@picker|path` marker,
  skipped in `commit_fields` like `@scrub|`, and a branch in `draft_records`.
- Paint: `Shell::paint_decorations` splices all decorations *under* `popup_start` (before the
  statusbar), i.e. under the popover. Picker draws must be spliced at the paint start of the first
  node after the popover subtree (`ui.paint_order`/`paint_starts`), shifted by the commands the
  earlier splices added, inside a clip of the popover rect.
- `Popovers::sync` focuses the popover's first field on open (the hex field).

## Rendering design (not started)
- Add `Draw::Fill { node, rect, radius, fill: Fill }` in `ara-ui/src/paint.rs` with
  `Fill::Linear { from, to, vertical, linear_light }`, `Fill::Hue { ring: Option<thickness>, s, v }`,
  `Fill::Checker { cell, light, dark }`.
- New `Pipeline` modes 9/10/11 in `ara-ui-render/src/gpu.rs` (`prepare`), CPU reference in
  `reference.rs`, `ara-ui-render/shaders/ui.slang` and `ara-editor-gpu/shaders/ui.slang`
  (re-cook `.spv` with `compile.ps1`). `ara-editor-gpu` passes `batch.pipeline as u32` as the mode.
  Update the exhaustive matches in `ara-ui-test/src/geometry.rs`.
- SV square = horizontal white→hue gradient plus vertical transparent→black overlay (exact
  V·lerp(1,hue,S) in sRGB display space). Ring = analytic AA annulus, hue by angle.
  Linear-mode tracks interpolate in linear and encode sRGB per pixel (`linear_light`).

## Schema (inspector agent owns)
Agreed with "UX: inspector fields": `EditorHints.hdr` and `EditorHints.temperature` (serde default
false), exposed as `inspector::Field.hdr`/`.temperature`. Annotate `MeshRenderer.emissive`,
`emissiveFactor`, `Light.color` (hdr) and `Light.color` (temperature). Snapshot gate:
`cargo test -p ara --test cli`.

## Interaction decisions
One undoable transaction per drag gesture; Escape applies a revert-to-Old transaction (only if
changed) and closes; Enter or click-outside commits. Pinned mode = `at='x,y'` popover, draggable
title strip, exempt from click-outside, editing the entity IDs captured when it opened.
