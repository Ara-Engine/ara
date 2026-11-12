# Findings from building Glint

Gaps hit while building a small rolling-ball game as a user of Ara (C# scripting, the
`ara` CLI, `ara capture`, `ara_play` over MCP). What was tried, what happened, what was
done instead. Severity: **P0** blocks the work, **P1** visibly wrong or slow, **P2**
friction. House style: [`../FINDINGS.md`](../FINDINGS.md). No engine code was changed.
Work in progress: the list grows with the game. Items marked *from reading* have not yet
been confirmed by running the game.

## What worked first time

- The Glint C# module (five files, 317 lines: components, two behaviours, `[Query]`
  systems, a helper class) compiled with no diagnostics on the first build.
- Entities carrying the module's components were accepted by `ara do` after the module
  compiled (`ara-samples build glint`, 1.5 s).
- A headless `ara_play` run steps the scripts: with `Move [0, 1]` held for 120 ticks the
  ball reached z = 12.2 m at 7.5 m/s, grounded, with the run clock at 122 ticks. Every
  `step` returns a state hash, which the replay will compare.

## P1

1. **Scripts have no contact events, ray casts or impulses.** `Movement` has `Stop` and
   nothing else for physics bodies. The ball is a dynamic sphere, so grounding is a test
   against the course's own `Surface` components (same half extents as their colliders)
   and steering writes `Velocity`. A level whose platforms are not axis-aligned boxes would
   need its geometry duplicated in script. *From reading the API; confirmed by what the
   controller had to do.*

## P2

2. **`ara_play read` takes `paths` (a list), not `path`.** Rejected with `ARA1101`
   (`PlayReadParams has no field path`), with a good fix hint. Worth knowing because the
   other `ara_play` actions' examples use single values.
3. **`ara init` leaves an empty `content/scenes/main.scene`** that `project.setMain` then
   makes redundant (also in Atelier).
4. **The `--game` shell is shaped around Outpost's co-op flow** *(from reading
   `crates/ara-player/src/game_app`, not yet run)*: Loading leads to a Lobby screen that
   needs a Ready click, and the shell's actions are fixed (`solo`, `host`, `join`, `ready`,
   `resume`, `leave`, `settings`, `quit`). Glint reuses the Lobby as a "start rolling" page.
   The input actions are fixed too (`Move`, `Look`, `Jump`, `Sprint`, `Interact`,
   `Inventory`): restart uses `Interact` (E).
