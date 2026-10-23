# ara-physics2d

Owned Rapier2D API. No Rapier type escapes the public engine boundary.
API: Body2d, Collider2d, Character2d, register, register_sim, Physics2d::new,
Physics2d::step, snapshot, restore, step_system, commands::register.
Physics2d is solver infrastructure, not authoritative gameplay. Components hold
positions/velocities/controller state; sync runs only at fixed tick boundaries.
Stable Identity order determines insertion, removal, controller and event order.
The complete solver (contacts, islands, broad phase, handles) is snapshotted via
the registered Physics2d resource; do not rebuild a world from transforms on rollback.
Snapshots have an explicit build fingerprint and bounded decoder. Cross-platform
enhanced determinism is enabled, but cross-platform equality still needs measurement.
Do not enable SIMD or parallel behind the same snapshot/build fingerprint.
Collider polygon points are local-space convex hull inputs. Tile colliders are
created as Body2d fixed entities through authoring commands before projection.
The system stores an error on the service resource for host diagnostics; hosts must
stop/fault play when Physics2d::error returns Some. Direct step returns Result.

Set CARGO_BUILD_JOBS=2. Gates: cargo check/test -p ara-physics2d;
cargo clippy -p ara-physics2d --all-targets -- -D warnings;
cargo fmt -p ara-physics2d -- --check.
