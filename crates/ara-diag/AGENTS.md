# ara-diag

Structured diagnostics for every Ara subsystem, the diagnostics bus that carries
them to the project host, and the documentation of every code. Leaf crate (serde
only); every crate can depend on it.

## Public API

- `Diagnostic { code, severity, message, location: Box<Location>, subjects, data,
  fixes }`. Builders: `error/warning/info(code, msg)`, `.file() .doc() .record()
  .path() .line_col() .command() .within(&base) .with_fix() .subject(s) .data(v)`;
  `.emit()` / `.emit_with(Meta)` put it on the bus. `Display` is rustc-style text.
- `Subject { kind, value }`: `entity:@levels/a.scene#e_8b21`, `asset:@meshes/x.glb`,
  `document:…`, `system:physics.step`, `pass:gbuffer`, `module:Gameplay`. Parses
  bare forms (`e_8b21`, `@path#id`); `matches` compares reference halves and entity
  addresses (`e_4f9a` matches `e_4f9a/e_m4tz`).
- `Code(u16)`: `ARA####`. `Code::parse`, `Code::doc`. Named codes (`ARA-EXP-*`,
  `ARA_GPU_*`) print and parse by name; their numbers are slots in 6000–6999 and
  8000–9999 (`catalog::label`).
- `Fix { title, action }`, `FixAction::{Set, Remove, Rename, Command, Call, Cli}`.
- `bus`: `admit(code)`, `push(d, Meta)`, `emit_with`, the `emit!` macro (admission
  before formatting), `Meta { cause, dedup, origin, tick }`, `EventId`, `Origin`
  (`engine`, `human`, `agent:<name>`), `scope(origin)`, `set_tick`, `set_burst`,
  `attach() -> Consumer` (`drain(&mut Vec<Event>) -> Lost`).
- `store::Store`: `push`, `ingest(events)`, `absorb(&Lost)`, `query(&Filter)`,
  `clear`, `get`, `by_id`, `causes`, `effects`, `cursor`, `expire`, `save` /
  `restore` (persistence), `Retention` (per severity, optional max age).
  `Filter { since, codes, severity, subjects, origin, from_ms, to_ms, from_tick,
  text, limit }`. `Record { seq, first, count, sampled, id, cause, origin, at,
  first_at, tick, diagnostic }`.
- `summary::summarize(&records, top_subjects) -> Summary` (groups by code with
  counts, subjects, newest message, best fix); `Summary::text(budget)`.
- `catalog`: `all()`, `get(code)`, `lookup(text)` (number, label, alias, constant
  name), `label(code)`, `CodeDoc { code, name, label, aliases, owner, title,
  explanation, causes, fixes }`, `CodeDoc::markdown()`.
- `codes`: kernel and bus constants; `codes::ALL`. `has_errors`, `did_you_mean`,
  `nearest`.

## The bus

- Any thread: `Diagnostic::…().subject(…).emit()`. Each thread lazily gets a 512-slot
  SPSC ring; a full ring drops and counts (reported as ARA2001). No locks on emit.
- Off until a `Consumer` attaches (the host does): `admit` is one relaxed load.
- Sampling: 32 events per code per thread per drain window (`set_burst` tunes it);
  the rest are only counted and added to the code's latest record (`sampled`).
- Emit cost (`cargo bench -p ara-diag --bench emit`, the Windows dev box, shared):
  bus off 0.4–0.9 ns, sampled out 5–7 ns, admitted push ~60 ns plus building the
  diagnostic (~100 ns with `format!`), host ingest ~0.5 µs per event.
- Event IDs are per process (thread slot << 32 | counter); restored records get IDs
  with the top bit set, and persisted causes link by dedup key.
- One consumer per process; with several, each event goes to whichever drains first.

## Code ranges

| Range | Owner |
|---|---|
| 0001–0899 | C# analyzers (0700–0799 generators) |
| 0900–0999 | C# host (`ara-script`) |
| 1000–1999 | kernel: 1000 JSON, 1100 schema, 1200 documents, 1300 references, 1400 prefabs, 1500 transactions, 1600 commands, 1700 merge, 1800 protocol, 1900 projects |
| 2000–2099 | the bus, `ara why`, runtime events (2020 reloads, 2030 frames) |
| 2100 / 2200 / 2400 | assets / world streaming / networking |
| 3100 / 3200 / 3300 / 3400 | graphs / materials / render visibility / audio |
| 3600–3699 | terrain, VFX, 2D, 2D physics (3600, 3601 collide: see below) |
| 6000–6099 | GPU, named `ARA_GPU_*` |
| 7000s / 7100 / 7400 / 7500 | physics / navigation / localization / saves |
| 8000–9999 | export, packs, bridge, named `ARA-EXP-*` |

## Documenting codes

- Every code has an entry in `codes/*.md` (one file per family). Format:
  `## ARA1302 NAME: Title`, `owner:`, optional `shown as:` / `aliases:`, the
  explanation, `Causes:` and `Fixes:` lists (one line per item). Strict parser.
- `tests/coverage.rs` scans `crates/*/src` and `dotnet/src` for `Code(n)`,
  `codes!` tables, `ARA-EXP-*`, `ARA_GPU_*` and C# `"ARA0nnn"`; it fails on an
  undocumented code or a number two crates define. Known collisions: ARA3600
  (terrain, VFX) and ARA3601 (VFX, 2D), documented as one entry each. Never add
  to that list; renumber the newer code.
- Never renumber or reuse a code. Messages put identifiers in backticks; prefer a
  `Fix` over prose when the edit is mechanical.

## Testing

`cargo test -p ara-diag`: JSON shape, rendering, subjects, ring (SPSC stress),
store (dedup, retention, causes, save/restore), summaries, catalog parsing and
lookups, `tests/bus.rs` (threads, sampling, drops; the bus is global, so its tests
take a lock), `tests/coverage.rs`. `cargo bench -p ara-diag --bench emit`.
