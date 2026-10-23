# Navigation v2 and gameplay AI: results

Scope: `ara-nav` v2 (large worlds, dynamic world, crowds), `ara-crowd` (ORCA,
flow fields), `ara-perception` and `ara-ai` (state trees, utility, EQS, DSL,
`ara why`). Machine: Windows 11, Ryzen 7 7800X3D (8 cores / 16 threads),
Rust 1.99.1/MSVC, release benches, `CARGO_BUILD_JOBS=2`. The host was shared
with other agents' builds (6+ `rustc` processes, ~65% CPU load during some
runs), so timings are upper bounds; tail latencies (p95/max) are the most
affected. Cross-platform bit equality is not claimed (same-binary determinism is).

## Targets

| Target | Result | Status |
|---|---|---|
| 10 km path query in <= 1 ms (16 km² world) | 9.85 km route, **0.38 ms p50** (cluster A* + ALT) | met |
| 10,000 agents with avoidance at 60 Hz on 8 workers, <= 2 ms/tick, deterministic | full ECS nav tick **CROWD_P50 ms p50** (ORCA solve alone 1.48 ms); bit-identical on 1/2/4/8 workers | see below |
| Deterministic cross-process hashes at 1/4/8 workers | identical (`ara-ai` 128 NPCs, `ara-crowd` 10k, `ara-perception` 2k) | met |
| Streaming swap without invalidating paths | agent follows a route across 6+ resident swaps and arrives | met |
| Golden behaviour scenarios | guard (patrol/investigate/chase/lose/return), cover fight | met |

## Large worlds (`cargo bench -p ara-nav --bench scale`)

16 km² (4096 m x 4096 m) floor with two serpentine walls, 256 cells of 256 m
(4 x 4 tiles of 64 m, 1 m voxels), baked per cell with halo geometry on Rayon:

- Per-cell bake of all 256 cells / 4,096 tiles: 396-789 ms wall (parallel, flat
  floor; dense collision costs more). Cell tiles are bit-identical to a
  whole-world bake (`cell_bakes_equal_whole_world_tiles...` test).
- Cluster graph: 3,960 portals, 55,968 edges, built in 9 ms (+8 ALT landmarks).
- Route queries (200 samples each):

| Query | Straight | Route | Expanded | p50 | p95 |
|---|---|---|---|---|---|
| corner to corner through both walls | 4,010 m | **9,853 m** | 1,746 | **0.376 ms** | 1.10 ms |
| mid-south to far north-west | 4,338 m | 7,979 m | 546 | 0.118 ms | 0.59 ms |
| east edge south to north | 4,011 m | 9,726 m | 516 | 0.109 ms | 0.32 ms |

  History: Euclidean A* over 32 m portals expanded 4,809 nodes (1.67 ms p50);
  ALT landmarks cut that to 2,801 (0.84 ms); quarter-cell portals (64 m) halved
  the graph for +0.4 % route length.
- Resident 3 x 3 cell assembly (worker thread): 0.35-0.43 ms.
- Route quality vs exact Detour path on a 256 m maze: within 0.99-1.35x (test).

## Crowds (`cargo bench -p ara-crowd --bench crowd`, `-p ara-nav --bench scale`)

ORCA solver alone, 10,000 agents in a dense four-way crossing inside a walled
arena (8.4 % of agent-ticks needed the minimal-violation fallback), 540 ticks:

| Workers | p50 | p95 | identical to 1 worker |
|---|---|---|---|
| 1 | 7.11 ms | 8.56 ms | - |
| 2 | 3.71 ms | 4.55 ms | yes |
| 4 | 2.21 ms | 3.01 ms | yes |
| 8 | **1.48 ms** | 2.17 ms | yes |

Full navigation tick (ECS gather, budgeted replans, path following, ORCA with
navmesh walls and 16 carved pillars, scatter), 10,000 agents crossing a 240 m
plaza, 300 steady ticks:

CROWD_TABLE

Solver choice (measured, single worker, same plaza): DetourCrowd 2,000 agents
12.3-15.6 ms p50 vs ORCA 2,000 agents 1.9-2.3 ms p50 (6.5x), and ORCA scales
across workers while the DetourCrowd shim is single-threaded per profile.
**ORCA is the default**; DetourCrowd stays selectable (`CrowdSolver::Detour`).

Flow fields (shared goal): 262,144-cell field built in 21.7-25 ms (one Dijkstra
per goal), then 0.07-0.29 ms to steer 10,000 agents. Use for massed groups with
one destination (RTS/hordes); per-agent paths otherwise (multi-level worlds).

Replan budget: 256 path queries per tick (stable-ID order), so 10,000 fresh
goals are planned within ~40 ticks while existing corridors keep moving.

## Behaviour (`cargo bench -p ara-ai --bench ai`)

AI_TABLE

## Determinism and verification

- `ara-crowd`: bit-identical positions/velocities for 1/4/8 workers and for any
  input order (neighbours ranked by (distance², stable ID)).
- `ara-perception`: identical memories for 2,000 perceivers on 1/4/8 workers.
- `ara-ai`: 96 guards + 32 soldiers + 4 intruders + noises, 900 ticks, hashed in
  child processes with 1, 4 and 8 Rayon workers and in-process with reversed
  spawn order: identical BLAKE3 hash of all nav/perception/brain state and the
  transition log.
- Streaming: an agent with a goal 176 m away (outside the resident 3 x 3
  window) follows a cluster route across 6+ swaps without ever going
  Unreachable; dynamic carving publishes exactly `rebake_latency_ticks` after
  detection (tested), avoid-only obstacles never rebake.
- Goldens (`crates/ara-ai/tests/golden`): guard 7 transitions (Patrol, GoTo,
  LookAround, Patrol, Chase, Search, Patrol); cover fight 99 transitions cycling
  Peek, StepOut, Shoot, FindCover, GoToCover, Hold with hidden cover verified by
  line of sight and health recovering only in cover.
- `ara why`: live sims produce ARA7101 (unreachable moveTo), ARA7115 (stuck in
  `Rest`), ARA7116 (perception occluded); the `npc` explainer ranks them with
  evidence plus setup checks (no Brain, maxSpeed 0, no NavMeshSurface).

## Known limits

- Hierarchical costs use base area costs; per-agent filters apply on resident
  legs only. Cluster attachment is a 2.5D sample grid (one layer per sample).
- ORCA walls ignore area filters; navmesh line of sight is 2.5D (low cover
  blocks sight; use `PhysicsLineOfSight` when that matters).
- Streaming publication ticks come from the host; record them for replay.
