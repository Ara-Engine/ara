# ara-perception

Deterministic AI perception: sight cones with pluggable line of sight, hearing
from one-tick noise events, and decaying memory. Depends on Bevy ECS, Rayon,
ara-schema (reflection) and ara-physics (ray line-of-sight adapter only).

## Public API

- Authoring (reflected, `register`): `Perceiver` (sight range, FOV, peripheral
  range, eye height, hearing, occlusion factor, memory seconds) and `Stimulus`
  (kind name, sight-target height, visibility multiplier).
- Runtime: `PerceptionPose { id, position, forward }` (stable ID + pose, synced
  by the host or `ara-ai` each tick) and `Perception { known, checks }`.
  `known` is the memory sorted by ID; `checks` the latest `SightOutcome`
  (Seen / OutsideCone / Occluded / OutOfRange) per candidate within 2x range.
- `NoiseEvents::push(Noise { source, position, radius, kind })` during the tick;
  `sense` consumes and clears them (empty at every boundary: snapshot-safe).
- `sense(&mut World, &dyn LineOfSight)` / `tick` (uses the `PerceptionLos`
  resource, else open space). `PerceptionConfig { fixed_dt }`.
- `LineOfSight` (`Sync`): `OpenSpace`, any `Fn(from, to, viewer, target) -> bool`,
  `PhysicsLineOfSight { physics, layer_mask }` (skips hits whose `user_data` is
  the viewer or target stable ID; sensors never occlude). `ara-ai` adds a
  navmesh-raycast LOS.
- `kind_hash(name)`: FNV-1a of stimulus kinds; behaviours compare hashes.

## Rules and gotchas

- Perceivers update in parallel over ECS table slices; each reads only the
  sorted target list and noise queue, so 1/4/8 workers give identical memory
  (tested). The LOS implementation must be deterministic and thread-safe.
- Memory keeps no global clock: `since` counts ticks since last sensed and
  `strength` decays linearly over `Perceiver::memory`; expired entries drop.
- Sight beats hearing: a noise never overwrites a position seen the same tick.
  Anonymous noises (`source == 0`) share one memory slot.
- Noise radius is scaled by `hearing`; occluded paths (LOS blocked) are heard
  only within `radius * occlusion`.
- Push noises before `sense` in the same tick (gameplay emits them alongside the
  presentation sound; audio playback itself is not deterministic simulation).
- Cost is O(perceivers x stimuli) distance checks plus one ray per candidate in
  range and cone. Add a spatial grid if stimuli number in the thousands.
- Use the same stable ID for `PerceptionPose`, `NavAgentState` and physics
  `PhysicsId`/`user_data`.

## Testing

`CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`:
`cargo test -p ara-perception`,
`cargo clippy -p ara-perception --all-targets -- -D warnings`,
`cargo fmt -p ara-perception -- --check`.
Tests: cone/peripheral/occlusion/visibility outcomes, hearing radius and
occlusion, memory decay timing, identical results on 1/4/8 workers.
