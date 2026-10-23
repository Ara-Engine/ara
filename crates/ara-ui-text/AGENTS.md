# ara-ui-text

Purpose: explicit font resources, reusable Parley contexts and grapheme-safe single-line editing.
No layout-tree, renderer, window, managed-runtime or ECS dependency belongs here.

Public API:
- `TextSystem::new(font_bytes, family_list)` registers explicit fallback resources.
- `TextSystem::bundled` ships pinned OFL Noto Sans and Noto Naskh Arabic.
- `Text` caches shaping separately from line breaking; surviving keyed rows keep this object.
- `TextSystem::measure` reshapes changed text/style and rebreaks changed widths.
- `inspect` reports bidi runs, original byte clusters and missing glyphs.
- `TextInput`, `EditEvent`, `Clipboard` provide selection, undo/redo and simulated/native IME seams.

Gotchas:
- Selection offsets are UTF-8 bytes at grapheme boundaries. OS adapters must convert their indices.
- IME preedit never mutates the committed value; commit replaces the saved selection in one undo step.
- Cursor geometry and visual bidi navigation are supplied by Parley in ara-ui.
- Clipboard access is caller supplied; no OS calls or per-frame model polling live here.
- Explicit fallback avoids dependence on platform font discovery. No installed Windows fonts are required.
- CJK/emoji/localization font packs are host resources, passed to `TextSystem::new`.
- Tests in ara-ui-render carry licensed corpus subsets; do not ship those subsets as complete font packs.
- Full bundled fonts and licenses are in `fonts/`; immutable source URLs and SHA-256 are in `sources.json`.
- Parley's `complex-scripts` feature enables ICU4X dictionary data for CJK/SEA word boundaries.
- No HarfBuzz/ICU C/C++ FFI. A new shaper requires a demonstrated coverage failure.
- Font rasterization formats belong to ara-ui-render; the tested color-emoji pack is CBDT/CBLC.

Tests: `CARGO_BUILD_JOBS=4 cargo test -p ara-ui-text`.
Run targeted Clippy `--all-targets -- -D warnings` and rustfmt check before completion.

Text.family is an optional explicit family list; a changed family invalidates
shaping through ara-ui. Hosts register all faces explicitly, including monospace.
