# ara-anim

CPU skeletal animation over immutable cooked assets, with caller-owned buffers.
The library has no world, scheduler, task pool, wall clock, global state or IO.
`AGENTS.md` lists its entry points and checks. All public APIs have Rustdoc.

## Runtime contract

- `Skeleton::cook` validates parent-before-child topology, stable joint IDs,
  positive scales, normalized rotations and retarget reference metadata. Explicit
  imported inverse binds are preserved; an empty inverse-bind list derives them.
- `CookedClip::cook` validates integer key times, sparse channels, property paths
  and stable notify IDs. Empty channels use bind pose. Translation, rotation and
  scale have independent linear/step interpolation modes. Quaternion linear
  interpolation means shortest-path nlerp. Reverse seeks are supported; gameplay
  clocks advance forward with an exact fixed integer step.
- `Sampler` caches each channel's key bracket and decompressed endpoints. An
  adjacent bracket reuses its preceding decoded endpoint. Translation, rotation
  and scale live in separate aligned SIMD streams (`Vec3A` / `Quat`).
- `Pose::blend` does linear/masked interpolation; `Pose::add` applies translation
  differences, quaternion differences and multiplicative scale ratios relative
  to a reference pose. IK returns model rotations, then applies them through the
  current parent transform. Recompute models between dependent constraints.
- `Controller::compile` resolves `@path#id` clips, validates all DAG nodes and
  conditions, and prepares topological state evaluation plans. 1D blends clamp
  outside their scalar axis. Directional 2D blends use a center and angular ring,
  barycentric interpolation inside each wedge and radial projection outside.
  Layers/additives accept masks and constant or float-parameter weights.
- State transitions test conditions in serialized priority order before advancing
  time. Active fades finish before another transition starts. Selected transition
  triggers alone are consumed. Exit ticks are earliest elapsed state times; an
  exit at duration waits for one complete cycle. Same named sync groups transfer
  normalized phase on entry and keep it aligned throughout the cross-fade,
  including across different state durations. An integer remainder prevents drift. All
  descendant clips sample the state's normalized phase, so blend trees stay in
  phase despite unequal clip lengths. Missing scalar curves contribute zero.
- Root motion is an ordered SE(3) displacement, including rotation through every
  crossed loop. It is blended with tree/layer/fade weights; additive root deltas
  compose after base deltas. A root-masked layer cannot move the root. Extraction
  restores the motion root's bind translation/rotation; animated scale remains.
  Apply the returned delta through the entity's preceding orientation in ara-sim.
- Events cover `(previous, current]`. Tick-zero notifies fire at loop boundaries,
  not on initial entry; nonlooping clips never repeat. Equal-time list order is
  preserved. Shared DAG leaf weights accumulate, delivering each occurrence once
  per lane. Cross-fades emit both lanes with end-of-tick weights. Layers weight
  their notifies by layer contribution independently of the bone mask; additives
  retain base event weight and add weighted overlay events. Event traversal is
  deterministic by compiled node order, then cycle/time/list order. Gameplay
  queues these callbacks into a batch; it must not call C# once per actor.

## Prediction and integration

**ara-sim:** store an `Animator` as owned per-entity ECS data, with a shared
`Controller` resource. Resolve `ParameterId`s once per program generation and
drive `ParameterValue::{Float, Int, Bool, Trigger}` through `set`. The Schema DB
registers parameter declarations, defaults and comparison types for generated C#
bindings and agents. Choose a fixed step/frequency matching the simulation (the
defaults are 800/48000, or 60 Hz). No internal thread pool competes with the ECS.

`Snapshot` contains program signature, both lane clocks, fade duration/progress,
simulation tick, sync phase remainder, all typed parameters and pending triggers. Use `snapshot_into`
with preallocated slots in the rollback ring. `restore` rejects incompatible or
invalid snapshots and rebuilds caches/pose/curves without redelivering events or
applying a root delta. Root delta consumption and gameplay event deduplication
belong to the same simulation snapshot. Replay is bit-exact for the same
binary/platform/configuration, not promised across different floating-point ISAs.

**ara-render:** after procedural pose edits, call `pose::model_matrices` and
`pose::skinning_palette` with preallocated `Mat4` slices. The palette is
`joint_model * imported_inverse_bind`, column-major. Mesh/world-space conventions
and the skinned mesh's model transform are the renderer/importer's responsibility.
GPU skinning does not run in this crate.

**ara-asset:** bake source skins/animations into `SkeletonData` and `ClipData` in
the importer worker. Flatten nonjoint ancestors into the skeleton's local bind
transforms and preserve the skin's actual inverse-bind accessor. Convert source
time to the integer clip frequency with a defined rounding policy. Bake glTF
CUBICSPLINE and spherical rotation interpolation to nlerp keys with a separately
measured resampling budget; the compression report bounds quantization of those
baked keys, not the original cubic/spherical curve or accumulated end-effectors.
The generated glTF fixture exercises real skin, inverse-bind and channel readers
without downloads. gltf is a dev dependency only.

Cook/cache `CookedClip::to_bytes` out of process; `from_bytes` validates every
packed range, bit rate, count, timestamp, quaternion and skeleton signature.
The binary handoff is `ARAANM`, version byte 1, zero byte, then postcard fields:
duration, frequency, skeleton signature, joint tracks, property curves, notifies.
Each optional channel has exact uint32 times, four float ranges, four component
bit rates, dimension, interpolation and a little-endian packed bitstream. Zero
bits means a constant component; 8–23 bits quantize within its own range; 32 bits
stores raw float32 when quantization cannot satisfy the bound. Cooked bytes are
derived cache data, never source documents. The parent workspace owns dependency
lockfile reconciliation; this track intentionally changes only this new crate.

**ara-graph:** source is `ControllerData`, with stable map keys for parameters,
nodes and states, and explicit stable transition IDs. Map pose nodes to typed pose
ports; `samples[*].node` becomes a stable input edge with its blend coordinate.
Clip nodes carry canonical asset references. State/transition records lower to
the same program; conditions remain typed parameter predicates, never opaque
scripts. The compiler is independent of graph canvas/layout data. Later graph
integration must preserve edge order, transition priority and IDs. The `.animation`
kind and `AnimAsset` record are schema-registered now. Timeline curves use the
kernel's `FieldPath`, not a separate property-addressing syntax.

**Authoring:** `document::register/read/write` use ara-schema, ara-doc and the
ara-json canonical writer, including reflection order and default omission.
`AnimationCommand` lowers create/replace/field intents to typed kernel edits;
`apply` enters attributed ara-ops transactions with automatic inverse ops. Cooking
is read-only with respect to those documents. Publish new immutable programs only
at a tick boundary, retaining the last good generation and honoring the human's
apply policy (D20). A new generation requires explicit parameter/state migration;
old snapshots are rejected rather than silently reinterpreted.

## Measured workload

Windows 11, Ryzen 7 7800X3D, Rust 1.99.1, release defaults, 2026-10-23.
Run `CARGO_BUILD_JOBS=2 cargo run -p ara-anim --release --example crowd -- 300`
(set the environment variable separately in PowerShell). This run used a
below-normal process priority. The shared host reported 61% CPU load during the
session; these results may include contention and are not isolated hardware limits.

1,000 actors × 65 bones; two 61-key clips, translation/rotation/scale, a scalar
curve and footstep notifies. Every state evaluates a 1D blend tree and the two
states repeatedly cross-fade with a sync group. Actors start at staggered phases
and varied blend parameters. Both lanes are evaluated; there is no identical-pose
shortcut. Includes notify processing, root extraction and snapshot buffer reuse.
Palette mode also includes hierarchical matrices and final skinning matrices.
30 warmup frames, 300 measured frames per row, Rayon chunks of 32 actors; setup,
cooking and task-pool creation are outside the timer. No GPU or ECS time included.

| Workers | Animation p50 / p95 (ms) | Animation + palette p50 / p95 (ms) |
| --- | --- | --- |
| 1 | 19.440 / 23.572 | 20.219 / 24.717 |
| 2 | 11.290 / 15.724 | 12.035 / 17.333 |
| 8 | 2.929 / 3.130 | 3.096 / 3.558 |

Different rows ran sequentially under changing host load. The first 200-frame
baseline before endpoint reuse, redundant normalization removal and inlining was
30.116 / 40.248 ms on one worker, and 6.910 / 12.176 ms on eight. An intermediate
300-frame pass measured 20.157 / 33.261 ms and 5.155 / 11.576 ms, respectively
(33% / 25% lower p50). The table is a final 300-frame rerun after sync-group review.
Host contention limits causal precision; do not attribute all final improvement
over the initial baseline to the code changes.

Compression: 143,796 source key bytes → 45,622 complete cooked bytes, **3.152×**.
Maximum measured errors over keys and 16 subdivisions of every source interval:
translation **0.039193 mm**, rotation **0.000010339 rad** (0.0005924°), scale **0**,
scalar **0**. Conservative all-time local bounds: translation **0.039230 mm**,
rotation **0.000244495 rad**, scale **0**, scalar **0.000061520**. Requested budgets
were 0.1 mm, 0.0005 rad, 0.0001 scale and 0.0001 scalar. The cooker rejects both
measured and conservative bounds exceeding the requested budget. These are local
component bounds; long chains need asset-specific end-effector checks.

The [ozz multithread sample](https://guillaumeblanc.github.io/ozz-animation/samples/multithread/)
describes per-character preallocation and parallel sampling/local-to-model jobs,
but provides no hardware-normalized millisecond table for this workload. Its
[runtime documentation](https://guillaumeblanc.github.io/ozz-animation/documentation/animation_runtime/)
also describes a different compression/layout strategy. No numeric speed claim
against ozz is justified by these sources. A controlled same-assets ozz benchmark
is still needed before claiming best-in-class throughput.

## Validation

`cargo test -p ara-anim`, `cargo clippy -p ara-anim --all-targets -- -D warnings`,
`cargo check -p ara-anim`, `cargo fmt -p ara-anim -- --check`.
Tests cover reference accuracy, measured/certified compression error, malformed
cache rejection, sparse/step/half-turn channels, masked/additive blends, IK,
palettes, transition conditions and trigger consumption, sync groups, multi-loop
root motion, equal-time/loop-boundary notifications, shared-DAG event weights,
canonical schema documents, typed undo and exact rollback replay. A standalone
allocator-instrumented test observes **zero allocations, reallocations or frees**
across playback, cross-fades, snapshot reuse, restore and palette generation.
