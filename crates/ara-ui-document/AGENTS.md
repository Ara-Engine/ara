# ara-ui-document

Purpose: original UTF-8 UI source, strict syntax, component expansion and designer ops.
No layout, font, GPU, window, scripting or ECS dependency belongs here.

Public API:
- `SourceDocument::parse`, `serialize`, `revision`, `command`, `apply`.
- `Command::{SetAttribute, Insert, Remove, Move, SetStyle}` produces `SourceOp`.
- `SourceOp::inverse` reverses all patches in post-operation byte coordinates.
- `parse` retains authored structure; `compile` expands templates and named slots.
- `stylesheet::Stylesheet` retains declaration/rule spans and global tokens.
- `Diagnostic` supplies ARA7000–ARA7099 UI codes, byte/line/column location and a fix.

Rules:
- Never serialize a runtime tree over authored source. `serialize` is the lossless writer.
- Commands validate their candidate before returning; application is atomic and revision checked.
- A move carries the original node bytes. Adjacent whitespace/comments remain untouched.
- Every element has a unique ID. Expanded component IDs are `instance/local-id`.
- Templates have one root. Instances project one child per named slot; empty slots may have defaults.
- Text uses an attribute, never mixed child content. Unknown tags/attributes/CSS are errors.
- Selectors are one type, class or ID with one hover/focus/active/disabled state.
- Global tokens are whole-value `var(--name)` references; cycles fail eagerly.
- Public syntax/op structs are serializable so the host can wrap them in Ara commands/transactions.
- Optional `test-id` metadata supports semantic queries; it is preserved through source edits/expansion.
- `SourceOp` supplies preimages/inverses; the engine transaction owner supplies author and why.
- The root diagnostic catalog can register the UI range when the shell integrates diagnostics.

Tests: `CARGO_BUILD_JOBS=4 cargo test -p ara-ui-document`.
Also run targeted Clippy with `--all-targets -- -D warnings` and rustfmt check.
Tests cover CRLF, Unicode, quote styles, comments, structural edits, inverses and stale conflicts.
