# ara-diag

Structured diagnostics for every Ara subsystem: stable `ARA####` codes, a severity,
a message, a structured location and machine-applicable fixes. Leaf crate (serde only).

## Public API

- `Diagnostic { code, severity, message, location: Box<Location>, fixes }`
  - builders: `Diagnostic::error/warning/info(code, msg)`, `.file()`, `.doc()`,
    `.record()`, `.path()`, `.line_col()`, `.command()`, `.within(&base)`, `.with_fix()`.
  - `Display` renders rustc-style text; serde gives the JSON agents read.
- `Location { file, doc, record, path, line, column, command }`: all optional.
- `Fix { title, action: FixAction }`, `FixAction::{Set, Remove, Rename, Command}`.
- `codes::*`: the catalog. `codes::ALL` lists `(Code, NAME, description)`.
- `has_errors(&[Diagnostic])`, `did_you_mean(input, candidates)`.

## Code ranges

| Range | Area |
|---|---|
| 0001–0999 | reserved for the C# analyzers (M1 C# host) |
| 1000 | JSON and files |
| 1100 | schema (types, fields, paths, upgrades) |
| 1200 | documents (IDs, parents, structure) |
| 1300 | `@path#id` references |
| 1400 | prefabs |
| 1500 | transactions and undo |
| 1600 | commands |
| 1700 | merge |

## Rules

- Never renumber or reuse a code. Add new ones at the end of their range, with a
  doc comment (it is the `ara explain` text).
- Messages put literal identifiers in backticks and say what to do when it is not
  obvious. Prefer a `Fix` over prose when the edit is mechanical.
- `Location.path` uses field-path syntax (`Transform.position`, `overrides[k]`);
  list elements show as `[#2]` (display only, never addressable).

## Testing

`cargo test -p ara-diag`: code uniqueness, JSON shape, text rendering.
