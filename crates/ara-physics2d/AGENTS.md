# ara-physics2d

Ara-owned 2D physics over Rapier2D (pure Rust). No Rapier type escapes the public API.
Mirrors ara-physics' 3D vocabulary with dimension-specific 2D geometry.

## Public API

- Components (module `physics2d`): authoring `RigidBody2d` + `Collider2d`, or
  `CharacterController2d` (owns its capsule); per-tick intent `CharacterInput2d`;
  mirrored state `PhysicsPose2d`, `Velocity2d`, `CharacterMotion2d`.
- `register(db)` / `register_sim(registry)`: schemas, ECS codecs and the `Physics2d` +
  `Physics2dStatus` resources (both carried by ara-sim checkpoints).
- `install(session, Physics2dConfig)`: inserts resources and appends the boundary system.
  `tick_ecs(world)` runs one boundary directly; `prepare_ecs` binds without stepping.
- `Physics2d`: `raycast`, `touching` (contacts + sensor overlaps, sorted identities),
  `snapshot`/`restore` (versioned, build-fingerprinted, bounded at 256 MiB).
- `commands::register`: `physics2d.set` (atomic body-or-character role) and
  `tilemap.colliders.generate` (owned, replaceable, undoable static hulls).

## Rules

- ara-sim `Identity` is the stable ID (3D uses PhysicsId because it has no ara-sim
  dependency). Extraction, creation, removal and characters run in Identity order.
- Gameplay state is ECS: pose, velocity, grounded. The solver (contacts, islands, BVH,
  handles) is infrastructure snapshotted whole; never rebuild it from transforms.
- Writes to `PhysicsPose2d` teleport static/dynamic bodies and characters; kinematic
  bodies move to it. Writes to `Velocity2d` apply to dynamic bodies.
- Character policy matches 3D CharacterVirtual: grounded drops vertical speed, jumps
  only from ground, gravity integrates the rest; desired velocity ⟂ gravity.
- New/teleported colliders reach queries through `BroadPhaseBvh::set_aabb`. Never call
  `PhysicsWorld::detect_collisions`: it skips island links and corrupts later steps.
- Static bodies are not written back each tick; bodies without a pose start at origin.
- Faults are sticky in `Physics2dStatus`; restoring an earlier checkpoint clears them.
- Enhanced determinism is on; never enable SIMD/parallel features behind the same
  snapshot fingerprint. Cross-platform equality is still unmeasured.

## Test

Set CARGO_BUILD_JOBS=2. `cargo test -p ara-physics2d` (includes a cross-process
determinism harness). `cargo clippy -p ara-physics2d --all-targets -- -D warnings`.
Timings: `cargo run -p ara-physics2d --example physics2d-timings [--release]`.
