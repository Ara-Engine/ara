# ara-runtime-net

Networked play (D19: Host+N, Dedicated+N) on the runtime over `ara-net`: real
Lightyear replication, owner prediction with rollback and remote interpolation, not
lockstep copies. Depends on ara-net, ara-runtime, ara-sim, ara-physics.

## Public API

- `NetMotor` + `Locomotion` (`SimulationFeature`): the predicted unit. Ground X/Z in
  mm, facing/orbit in millidegrees, speed in mm/tick; `Move`, `Look`, `Sprint` and
  `Jump` share one native history entry. Steering follows camera yaw, using integer
  rotation math; vertical motion is predicted and corrected from Jolt. Corrections are
  reported in millimetres.
- `NetPlugin` (`RuntimePlugin`): registers `NetMotor` (every peer: the schema hash
  covers it); `net.motors` (Gameplay) steps motors from each player's movement actions
  and asks the physics character for the same velocity; `net.pin` (Late) writes motor
  X/Z and yaw into `Transform`/`PlayerControl` (height stays physical; the next
  `physics.push` teleports the body to match).
- `avatars(workspace, scene, players)`: copies the player-1 record as `e_player<NN>`
  for players 2..=N (1.5 m apart, fresh `PhysicsId`, no camera). Every peer applies it
  to the same snapshot before building, so identities match across processes.
- `attach_motors(runtime)`: inserts motors on `PlayerControl` entities (authorities).
- `NetHost::new(runtime, HostConfig { dedicated, clients })`: the runtime as an
  `ara_net::Authoritative` inside `Server<Locomotion>`. Listen hosts are player 1;
  clients are players 2.. Each client gets a `Link` proxy (optional `Conditions`,
  measured bytes). `join(index)` → `(player, endpoint, JoinToken)`; `poll()` every
  frame (the server's timeline steps the runtime through
  `Runtime::step_with_adapter`, plugins included); `input(after, injections)` for the
  listen player; `runtime()`/`runtime_mut()` between polls; `stats()` (`HostStats`:
  begin time, field payload, datagram bytes up/down, drops).
- `NetMirror::connect(runtime, player, endpoint, token)`: a client's mirror world,
  built from the same snapshot and never stepped. Its follow cameras retarget the
  client's avatar. `frame(runtime)` polls and, per new predicted tick, writes the
  owner's predicted motor, other players' interpolated motors and every other
  replicated entity's interpolated `Transform`/`Velocity`, then calls
  `Runtime::present_mirror` (animation, cameras, world poses). `input(after, ..)`
  (all four movement actions are networked), `input_source(fn)` for bots, `stats()` (`MirrorStats`:
  join time, rollbacks, prediction error mean/max in mm).

## Used by

`ara-player`: `play.start` with `Start.net` builds the world in its role
(`net::Play`), `play.join` issues client tokens (owners pass them over the trusted
player link only; never logged), status heartbeats carry `HostStats`/`MirrorStats`.
`ara-play-host` launches the authority first, then each client with its join;
`play.capture`/`read`/`inspect` take `player` to address a client process.

## Gotchas

- Networked worlds run on the authority's timeline: step/until/pause/patch/replay
  are local-only. The server starts ticking 120 ticks after the last client is ready.
- `JoinToken` is private launch data; never put it in results, logs or replays.
- C# commands run on the authority only. Reflected C# columns pass through the same
  Relevant/Owner/quantization policies as Rust columns; mirrors merge partial values.
  Policies and mirror component names are fixed at connection; schema-changing
  C# reloads require restarting the network session.
- The feature compatibility version is 2; old movement peers are rejected.
  Scene contacts still come from authoritative Jolt corrections.
- The mirror still builds the full world (physics, navmesh, C# host for the schema
  hash); only the session never steps.
- Scene/cell rows have one lifecycle owner: projection/streaming. A baseline that
  arrives first waits for its local row; only runtime-created entities get network
  fallbacks. Bindings are re-resolved after local cell unload/reload.

## Testing

`cargo test -p ara-runtime-net` (motor math). Process-level host + client with
captures: `cargo test -p ara-player --test net -- --nocapture`.
`cargo clippy -p ara-runtime-net --all-targets -- -D warnings`.
`mirror::entities` tests both baseline/cell arrival orders, unload/re-entry and
runtime-created fallbacks against the strict snapshot identity check. Its ignored
`profile_resident_replica_bindings` probe measures the binding hot path.
