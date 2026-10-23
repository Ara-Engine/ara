# ara-anim-physics

Narrow Jolt adapter for ara-anim. No authoring document writes, clocks, ECS scheduling
or renderer dependency. Profiles and typed authoring commands live in ara-anim.

## API

- `JoltGround`: allocation-free closest-ray ground probes with caller-supplied filtering.
- `JoltRagdoll::spawn`: boundary-only capsule body/constraint creation in profile order.
- `JoltRagdoll::bind`: short exclusive backend borrow for powered forces and hit impulses.
- `JoltRagdoll::destroy`: explicit boundary-only teardown, constraints before bodies.

## Contract

Runtime stores PhysicalState in the actor ECS component and keeps native Jolt snapshots
alongside the matching ara-sim snapshot. Restore both before resimulation. Runtime handles
are derived world-local bindings, never canonical document data or replicated IDs.
PD force/torque drives use existing Jolt constraints, not a parallel physics solver.
Shape/body setup allocates; successful warmed-up probes, reads, drives and blends do not.
The host configures ragdoll collision layers; self-collision policy is explicit at spawn.

## Checks

Set CARGO_BUILD_JOBS=2. Run cargo test -p ara-anim-physics,
cargo clippy -p ara-anim-physics --all-targets -- -D warnings,
and cargo fmt -p ara-anim-physics -- --check.
Replay tests run powered/hit/death/get-up through real Jolt at 1/4/8 threads,
with same-world and fresh-world snapshot restore and thread-count comparisons.
