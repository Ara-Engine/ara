# ara-anim

Deterministic CPU skeletal animation. Owns source schemas, checked cooking,
quantized clips, sampling, poses/IK, blend trees and state machines. No ECS,
renderer, file IO, clock, task pool or importer in the production library.

## API

- `data`: reflected source skeletons, transforms, clips, curves and notifies.
- `document`: schema/kind registration, canonical documents and typed op edits.
  Records are addressable: one `.animation` holds several clips/controllers and
  references name records (`@anim/fox.animation#an_walk`).
- `commands::register`: shared CLI/MCP/editor commands over `AnimationCommand`,
  including record creation, typed setters and arbitrary property edits.
- `Skeleton::cook`, `CookedClip::cook`: validate off-thread; publish at a tick boundary.
- `Pose`, `Sampler`: preallocate, then sample without allocation.
- `Sampler::sample_prefix`: opt-in presentation bone LOD; excluded joints use bind.
- `pose`: masked/additive blends, model/palette matrices, root deltas.
- `ik`: analytic two-bone and look-at constraints.
- `ik::Chain`: FABRIK/CCD, bind-local cone/twist/hinge projection; fixed iterations.
- `ik::FullBody`, `FootPlacement`, `HandGrip`: priority-ordered coupled contacts,
  shared hips, world-space stable ground probes and prop-relative grips.
- `retarget`: semantic humanoid roles, checked automatic name/topology mapping,
  bind-frame runtime retargeting, IK plant correction and offline clip baking.
- `matching`: normalized SIMD feature databases/caches, explicit candidate budgets,
  spring trajectory prediction and velocity-preserving inertialization.
- `Controller::compile_with_matching`: resolves motion databases and clips together.
- `physical`: bounded powered-body drives, fixed-tick hit/ragdoll/death/get-up blend
  state; narrow backend traits implemented by ara-anim-physics for Jolt.
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
- Cache version 2 strips defaults, packs exact uniform times and selects component/
  segment rates, with optional lossless integer prediction. Loading derives
  four-track time-major packets, then frees cooker
  arrays. `CookedClip::runtime_bytes` includes resident packets and encoded bytes.
- Cursors reuse endpoints; clipped/prefix sampling and compatible clip switches
  allocate nothing. Keep authoritatively consumed animation at the fixed tick rate.
- No tick allocations, clones, locks, wall clocks or hidden task dispatch.
- Motion-node playback/history/inertial offsets are authoritative snapshot state.
  Preallocate snapshot storage; its clone_from reuses all pose/offset vectors.
- SIMD partial searches report exact=false and visited; zero budget scans every row.
- Ragdoll PhysicalState belongs in the actor ECS component; native body snapshots
  must be restored with the same ECS boundary. Adapter handles are world-local caches.
- Contact priorities are ordered projection sweeps, with larger values solved last;
  conflicting goals have residuals, not an undocumented global optimality guarantee.
- Root extraction uses SE(3) deltas across every loop, before root removal.
- Events use (previous, current], including tick zero at a loop boundary; initial
  tick zero is not emitted. Cross-fades emit both lanes with explicit weights.
- Graph evaluation order and transition priority are serialized vector order.
- Authoring changes return ara-doc edits and enter ara-ops transactions.
- See README.md for cooked handoff, graph mapping and benchmark methodology.
- See V2.md, RESULTS.md, examples/documents and captures/v2 for v2 handoff/evidence.
