# ara-ai

Gameplay AI: hierarchical state trees with utility selection, EQS spatial
queries, perception-driven conditions, a name-based text DSL stored as an
`ara-graph` document, typed undoable commands, intents, a debugger stream,
capture overlays and `ara why` for NPCs. Depends on ara-nav, ara-perception,
ara-graph/cmd/doc/ops/schema/json, ara-diag, ara-why, ara-sim (Identity).

## Public API

- `dsl::{parse, reconcile, print, parse_expr}`: text <-> `ast::Behavior`. No IDs
  in text; `reconcile` keeps graph IDs by name and warns (ARA7114, with a
  `behavior.rename` fix carrying `restoreId`) on plain-text renames.
- `projection::{to_graph, from_graph}`: graph domain `behavior`, one node per
  var/query/state, expressions stored as canonical DSL text, transitions by
  target node ID. Editor layout survives every edit.
- `commands::register`: `behavior.text.apply`, `.rename`, `.state.add`,
  `.state.remove`, `.transition.add`, `.var.set`; all via `apply_snapshot`.
- `compile(&Behavior) -> Program` (ARA7110-7113 diagnostics, all at once).
- `register_sim(world, schedule, AiConfig)`: `sync` -> `perceive` -> `think` ->
  `bridge` before `NavigationTick`; `locomotion` -> `why::report` after it.
  `AiWorld::{load, insert_program, set_sight, trace, errors}`, `view(world, e)`,
  `bundle(id, key, position)` for spawning AI agents.
- Components: `Brain` (authoring: behaviour ref, `name = const` overrides),
  `BrainState` (complete runtime state), intents `MoveIntent`, `LookIntent`,
  `ActionIntent`, `LocomotionIntent` (animation/motion matching reads these).
- `NavLineOfSight` (navmesh walls), `overlay::{collect, svg}`,
  `why::{report, diagnose, NpcExplainer}` (register with `Why::register`).

## Semantics (see behaviors/*.behavior)

- Top-level states: implicit root, select first, no re-evaluation. Write an
  explicit root with `select first every N` for a priority selector.
- Each tick: transitions leaf-first then in order; else periodic re-selection
  (`every`); one state change per tick; then tasks of all active states, root
  first. Utility switches need `HYSTERESIS` (1.1x + 0.01). Children with no
  qualifying `when` leave the parent as the leaf.
- `on done`: every finishing task (moveTo, wait, find, act without `every`)
  succeeded. `on failed`: any task failed. Intents are rewritten every tick.
- `heard()` = a noise within the last `HEARD_WINDOW` (1 s); `sees(e)` = this
  tick; `lost(e)` = seconds since sensed. Entity positions are true poses.
- Overrides are constants only. Programs are keyed by graph asset ID; a new
  hash restarts brains at the next tick (hot reload at a tick boundary).

## Determinism and gotchas

- Brains run in parallel chunks; each reads only its own state and immutable
  inputs; trace/errors are merged by brain ID. RNG state lives in BrainState.
  Cross-process hashes match for 1/4/8 workers and any spawn order (tested).
- Publish navmeshes before the first AI tick (otherwise `navigable()` fails).
- Diagnostics: ARA7101/7102/7115/7116/7117 every `report_seconds` by brain age
  (free without a bus consumer). Catalog entries live in ara-diag/codes.

## Testing

`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`: `cargo test -p ara-ai`,
`cargo clippy -p ara-ai --all-targets -- -D warnings`,
`cargo fmt -p ara-ai -- --check`. Goldens: `tests/golden/*.txt`; regenerate
deliberately with `ARA_UPDATE_AI_GOLDENS=1` and review the diff.
`ARA_WRITE_CAPTURES=1` writes `captures/cover-fight.svg`.
Tests: DSL round trips/errors, commands + undo, guard and cover-fight goldens,
cross-process determinism, `ara why` causes, overlays.
