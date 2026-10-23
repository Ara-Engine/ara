# ara-anim

Deterministic CPU skeletal animation. Owns source schemas, checked cooking,
quantized clips, sampling, poses/IK, blend trees and state machines. No ECS,
renderer, file IO, clock, task pool or importer in the production library.

## API

- `data`: reflected source skeletons, transforms, clips, curves and notifies.
- `document`: schema/kind registration, canonical documents and typed op edits.
- `Skeleton::cook`, `CookedClip::cook`: validate off-thread; publish at a tick boundary.
- `Pose`, `Sampler`: preallocate, then sample without allocation.
- `pose`: masked/additive blends, model/palette matrices, root deltas.
- `ik`: analytic two-bone and look-at constraints.
- `graph`, `Controller`, `Animator`: typed parameters, compiled blend trees,
  ordered transitions, cross-fades, sync groups, event delivery and root motion.
- `Animator::snapshot_into` / `restore`: prediction boundary. Store animator state
  in an ara-sim ECS component; caches are derived, never authoritative globals.

## Checks

Set `CARGO_BUILD_JOBS=2`. Run `cargo check -p ara-anim`,
`cargo test -p ara-anim`, `cargo clippy -p ara-anim --all-targets -- -D warnings`,
`cargo fmt -p ara-anim -- --check`.
Run the explicitly requested performance workload with
`cargo run -p ara-anim --release --example crowd -- 200`.

## Invariants

- Source time is integer ticks; caller supplies a fixed integer step.
- Same binary/platform/configuration replay; no cross-platform bitwise promise.
- Skeleton parents precede children. Positive scales; normalized quaternions.
- Clip tracks and skeleton joints share an explicit cooked skeleton signature.
- Pose arrays are separate SIMD translation, quaternion and scale streams.
- Clip data, graph programs and skeletons are immutable; all scratch is per actor.
- No tick allocations, clones, locks, wall clocks or hidden task dispatch.
- Root extraction uses SE(3) deltas across every loop, before root removal.
- Events use (previous, current], including tick zero at a loop boundary; initial
  tick zero is not emitted. Cross-fades emit both lanes with explicit weights.
- Graph evaluation order and transition priority are serialized vector order.
- Authoring changes return ara-doc edits and enter ara-ops transactions.
- See README.md for cooked handoff, graph mapping and benchmark methodology.
