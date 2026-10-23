# Ara 2D physics

Rapier2D is behind an Ara-owned API. No Rapier handle/type enters components,
commands, scene files or caller queries. `Body2d` is the authoritative world-space
pose/velocity; `Collider2d` carries convex geometry, filters and physical material;
`Character2d` carries requested movement and grounded state. The renderer extracts
Body2d poses directly. Physics state never lives in C# objects or global state.

Register schemas with `register`, authoring with `commands::register`, then install
`register_sim` before projecting scenes. Insert `Physics2d::new(hz, gravity)` into
the session and register `step_system` after gameplay producers. A host must check
`Physics2d::error()` after each tick and fault play on failure, restoring a complete
session checkpoint before continuing. Exclusive callers can use fallible `step`.
`raycast` returns stable identities and excludes sensors unless requested.

All bodies are validated before changes and synchronized in stable Identity order.
The solver keeps contact/island/handle caches as infrastructure. `ara-sim` snapshots
register the entire resource alongside components, so rollback does not reconstruct
an approximate world from transforms. The direct binary snapshot also carries a
format version, actual Rapier version, precision, determinism policy, OS/architecture
and tick rate; restore validates before replacing the existing solver. Snapshot
decoding is bounded at 256 MiB. Netcode must authenticate snapshots and include the
complete application build/features hash (as ara-sim's checkpoint contract does).

## Backend choice

Both candidates are credible. Rapier provides [enhanced determinism](https://rapier.rs/docs/user_guides/templates/determinism/)
and [Serde solver serialization](https://www.rapier.rs/docs/user_guides/templates/serialization/).
Box2D's current documentation also includes [snapshots](https://box2d.org/documentation/group__snapshot.html)
and a [character mover](https://box2d.org/documentation/md_character.html); lack of
snapshot support is not a reason to reject it.

Rapier is selected because it is pure Rust, has a stateless kinematic controller
and exposes the complete solver for an owned, versioned rollback boundary. It needs
no C ABI shim or vendored C revision/build maintenance. Enhanced determinism is
enabled; SIMD and parallel execution are not enabled by this crate. Tests measure
same-build Windows determinism, controller grounding/wall blocking, CCD and full
session restore. Cross-platform hash equality remains unmeasured. Moving-platform
and stair behavior must be qualified for a game's controller settings, as with S6;
this is not a claim of Jolt's 3D controller parity.

No Box2D comparative performance measurement was made. `measurements/solver.json`
records ECS-inclusive Rapier timings and direct checkpoint sizes/costs. The
`solver-before-direct-snapshot.json` baseline measures the earlier nested-byte
envelope. Reproduce with `CARGO_BUILD_JOBS=2 cargo run -p ara-physics2d --example physics2d-timings`.

`physics2d.set` updates a complete body/collider/controller through typed ops.
`tilemap.colliders.generate` creates/replaces only entities bearing its owner marker;
the entire generation can be undone. Grid cells merge into rectangles; hex/diamond
cells retain their convex silhouettes. The command's origin is explicitly world
space, and its optional basis preserves rotation/scale/shear, so authoring hosts
must supply the map's resolved origin and basis.
