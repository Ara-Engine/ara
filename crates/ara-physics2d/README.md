# Ara 2D physics

Rapier2D behind an Ara-owned API that mirrors `ara-physics` (3D). Rigid bodies are
`RigidBody2d` + `Collider2d`; characters are `CharacterController2d` capsules driven by
`CharacterInput2d`. The solver writes `PhysicsPose2d`, `Velocity2d` and
`CharacterMotion2d` back at every fixed boundary. The renderer extracts
`PhysicsPose2d` directly. No physics state lives in C# objects or globals.

## Integration

Register schemas with `register`, authoring with `commands::register`, and
`register_sim` before projecting scenes. Call `install(session, config)` after the
gameplay systems that write inputs. The host checks `Physics2dStatus` after each tick
and pauses that session on a fault; restoring a checkpoint taken before the fault
clears it. Exclusive callers can use the fallible `tick_ecs`.

ara-sim checkpoints include the `Physics2d` resource: the complete Rapier world
(contacts, islands, broad phase, handles) plus stable-identity bindings, behind a
build fingerprint (format, Rapier version, precision, determinism mode, OS/arch)
and the world configuration. Rollback therefore restores the exact solver, not an
approximation rebuilt from transforms. Netcode must still authenticate snapshots and
include the application build hash, as ara-sim's checkpoint contract requires.

## Backend choice: Rapier2D over Box2D v3

| Criterion | Rapier2D 0.36 | Box2D v3.2 |
|---|---|---|
| Determinism | `enhanced-determinism` (cross-platform IEEE mode); bit-exact here across processes and insertion orders | Cross-platform determinism advertised since v3.1 |
| Snapshot/restore | Serde of the complete world; versioned by us | `b2World_GetSnapshot` exists but is marked experimental with an unstable format |
| Character control | Kinematic controller: slopes, autostep, snap-to-ground | Character mover primitives (plane solving, casts) |
| Performance | 0.6 ms/tick for 1k boxes (dev profile, ECS sync included) | Likely faster (SIMD solver); not measured here |
| Maintenance | Pure Rust, Cargo only, no ABI | C via an owned `cc` shim like S6 (Jolt), ~vendored revision upkeep |

Rapier wins on the two netcode-critical criteria: a stable, complete serialization
path we version ourselves, and no foreign ABI to maintain for a secondary feature
(D8). Its known costs are larger snapshots (≈1.8 MB for 1k bodies) and a slower
restore than Jolt (S6). S6's 3D moving-platform failure is avoided by our own
controller policy; 2D moving platforms (ground velocity) are not implemented yet.

## Measurements

`measurements/solver.json` (dev profile) and `solver-release.json` (release) hold
ECS-inclusive step, snapshot size and snapshot/restore costs for 129 and 1025 bodies.
Reproduce with `cargo run -p ara-physics2d --example physics2d-timings [--release]`.

## Authoring

`physics2d.set` writes a complete role (pose + body + collider, or pose + character)
and removes the other role's components in the same transaction.
`tilemap.colliders.generate` creates or replaces only entities carrying its owner
marker; the whole generation is one undo item. Grid cells merge into rectangles;
hex and diamond cells keep their convex silhouettes. The command's origin is world
space and its optional basis preserves rotation, scale and shear, so authoring hosts
must supply the map's resolved origin and basis.
