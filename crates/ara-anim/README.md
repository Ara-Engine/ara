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
- `Sampler` groups channels by exact timeline and interpolation mode, then samples
  four tracks per SIMD packet (`glam::Vec4`). Packed keys are time-major within
  packets. Shared timelines need one bracket/fraction calculation; uniform times
  use arithmetic addressing. Cursors reuse adjacent decoded endpoints. Constants
  are preclassified, and bind/default channels never enter the dynamic loop.
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
The binary handoff is `ARAANM`, version byte **2**, zero byte, then postcard fields:
duration, frequency, skeleton signature, joint tracks, property curves, notifies.
Version 1 caches must be recooked; source schemas and playback APIs are unchanged.
Each channel has exact single/uniform/explicit integer times, interpolation, a
fixed optional omitted quaternion component, and whole-track or 8/16/32-key
segments. The cooker chooses the smallest complete representation. Components
use zero/one/constant tags, 1–23-bit range-reduced integers, or raw float32 when
needed. Rates and ranges vary by component and segment. Optional lossless integer
prediction has independent delta rates; loading restores the exact quantized
integers before packet creation, adding no error or per-frame work. Dominant quaternion
reconstruction is used only with a certified range; half-turn ties keep full
precision. Exact bind channels are stripped. Both scalar and production SIMD
sampling are measured at source keys and 16 subdivisions, alongside conservative
all-time local bounds. No temporal key reduction or relaxed budgets are hidden.

Loading validates before deriving packets. Runtime releases the cooker track
arrays, retaining the versioned blob for `to_bytes`, root/curve metadata and
compressed SIMD packets. `runtime_bytes()` includes these owned allocations;
it does not confuse serialized compression with resident storage. There are no
expanded float key arrays. Cooked bytes are derived cache data, never documents.

`Sampler::sample_prefix` is opt-in presentation bone LOD: a parent-closed prefix
samples normally, excluded joints use bind pose, and scalar curves still sample.
Choose the prefix from screen size outside this crate. A presentation-only caller
can also sample every N fixed ticks and hold its previous pose between calls;
this update-rate LOD is opt-in and carries its own visual error. `Animator::tick`
continues evaluating every simulation tick, preserving root motion, gameplay
curves, notifies and rollback semantics. All reported measurements use full bones
and full rate. No internal scheduler or task pool is introduced.

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

## Measured workload: performance pass

Windows 11, Ryzen 7 7800X3D (8 cores / 16 threads), Rust 1.99.1, release defaults,
2026-10-23. Ozz was built locally with MSVC 19.50, release SSE2, at
[`744eb9d`](https://github.com/guillaumeblanc/ozz-animation/tree/744eb9d99f606eda849acb0b1204f7a3dc20bca1).
The parent implementation (`4e01c275`) was rebuilt against the same timing
harness and dependencies. Both Ara builds use 64-byte aligned actor headers,
contiguous chunks of 16, and caller-owned Rayon pools; runtime introduces no pool.

Every cell below is **p50/p95 milliseconds (other-process CPU load range)**.
Each percentile is the median of three 300-frame repetitions after 30 warmup
frames; the sixteen-worker controller rows have six repetitions after noisy
runs were repeated. A/B/C order reverses between repetitions. Background load is
measured separately from the benchmark's CPU consumption, over process/row windows
including setup and warmup. Exact readings accompany every individual timing in
[stages.jsonl](benchmarks/stages.jsonl), [crowd.jsonl](benchmarks/crowd.jsonl) and
[chunks.jsonl](benchmarks/chunks.jsonl). These are shared-host observations;
changing load and scheduler tails limit narrow comparisons. Setup, cooking,
accuracy checks and pool creation are outside all timers. Run details, source
and executable hashes are in [benchmarks/](benchmarks/README.md).

### Complete controller

1,000 actors x 65 bones, two 61-key clips, a scalar curve and footstep notifies.
Every state evaluates a two-input 1D blend tree; recurring cross-fades evaluate
both lanes with sync groups. Actors have staggered phases and varied blend
parameters. Includes notify delivery, root extraction and snapshot-buffer reuse.
Palette mode adds hierarchical model matrices and final skinning matrices.
All bones update every tick; presentation LOD is disabled.

| Workers | Mode | Ara before | Ara after |
| --- | --- | --- | --- |
| 1 | animation | 19.758/23.785 (24-85%) | 9.097/14.169 (43-84%) |
| 1 | animation+palette | 22.311/30.921 (29-75%) | 10.006/13.742 (42-78%) |
| 2 | animation | 11.526/13.699 (21-64%) | 4.236/5.096 (45-70%) |
| 2 | animation+palette | 11.918/15.083 (33-75%) | 4.839/5.919 (29-85%) |
| 8 | animation | 3.237/3.969 (24-60%) | 1.158/1.671 (46-54%) |
| 8 | animation+palette | 3.289/7.520 (33-60%) | 1.272/1.518 (13-76%) |
| 16 | animation | 2.407/4.312 (32-69%) | 1.042/1.843 (45-72%) |
| 16 | animation+palette | 2.815/6.948 (38-66%) | 1.104/1.496 (23-72%) |

Animation p50 improves 2.2-2.8x on these repeated controller runs. Each row is an
independent pass under its recorded load; differences between rows do not measure
a stage's incremental cost. There is no GPU, ECS or C# time in these numbers.

### Same-assets ozz kernels

The Rust exporter supplies the exact source floats and skeleton to the C++
adapter. Ozz's builder reorders joints depth-first; the adapter remaps original
IDs, clip tracks and inverse binds. Ozz pads to 68 SIMD lanes. Both harnesses
sample four independent clip instances, perform two lane blends and a final
cross-fade, and run equivalent local-to-model/palette operations. These skeletal
kernel rows exclude controller logic, root extraction, scalar curves, events and
snapshots in both implementations. Isolated stage medians are not additive.
Ozz uses its default AnimationBuilder, without AnimationOptimizer/key removal.

**1 worker(s)**

| Stage | Ara before | Ara after | ozz |
| --- | --- | --- | --- |
| sample (four clips) | 25.317/28.532 (43-68%) | 7.026/8.534 (29-73%) | 12.058/14.870 (45-83%) |
| blend (three pairs) | 0.718/1.247 (28-85%) | 0.700/1.141 (25-87%) | 0.622/0.983 (20-79%) |
| local-to-model | 0.516/0.748 (42-62%) | 0.448/0.791 (39-63%) | 0.392/0.749 (32-64%) |
| palette | 0.320/0.461 (36-66%) | 0.231/0.313 (28-72%) | 0.267/0.501 (26-61%) |
| combined kernels | 29.085/36.521 (33-73%) | 10.458/13.460 (51-67%) | 16.632/34.217 (44-77%) |

**8 worker(s)**

| Stage | Ara before | Ara after | ozz |
| --- | --- | --- | --- |
| sample (four clips) | 3.338/4.037 (30-79%) | 1.154/1.429 (39-70%) | 1.651/1.993 (42-77%) |
| blend (three pairs) | 0.184/0.280 (36-77%) | 0.189/0.249 (39-79%) | 0.146/0.180 (38-64%) |
| local-to-model | 0.113/0.157 (38-78%) | 0.114/0.143 (33-77%) | 0.101/0.128 (48-85%) |
| palette | 0.070/0.097 (46-97%) | 0.070/0.096 (31-74%) | 0.074/0.093 (34-70%) |
| combined kernels | 3.736/4.328 (40-69%) | 1.455/2.013 (47-65%) | 1.993/2.740 (38-65%) |

**16 worker(s)**

| Stage | Ara before | Ara after | ozz |
| --- | --- | --- | --- |
| sample (four clips) | 2.905/3.582 (36-55%) | 0.959/1.262 (43-50%) | 1.418/1.933 (40-48%) |
| blend (three pairs) | 0.176/0.275 (34-53%) | 0.195/0.320 (33-59%) | 0.116/0.142 (36-59%) |
| local-to-model | 0.105/0.159 (44-60%) | 0.106/0.165 (42-54%) | 0.083/0.107 (36-48%) |
| palette | 0.068/0.111 (34-66%) | 0.072/0.100 (32-49%) | 0.070/0.097 (36-58%) |
| combined kernels | 3.130/4.226 (38-44%) | 1.228/1.705 (41-44%) | 1.436/1.779 (33-43%) |

Ara wins sampling and combined-kernel p50 in this fixture. Ozz retains faster
isolated blend and local-to-model kernels. This pass changes decoding, sampling,
root endpoint reuse and controller evaluation; isolated blend/matrix arithmetic
is unchanged. Results apply to these assets, compilers and host conditions.

### Compression and precision

Ratios count **complete versioned cache blobs**, including metadata, curve and
notifies. Source bytes count float32 keys and uint32 timestamps, including
constants, matching the original benchmark definition. Requested budgets remain
0.1 mm translation, 0.0005 rad rotation, 0.0001 scale and 0.0001 scalar.

| Clip multiplier | Source bytes | Cache bytes | Ratio | Resident bytes | Measured translation / rotation |
| --- | --- | --- | --- | --- | --- |
| 1.0 | 143,796 | 12,051 | 11.932x | 45,059 | 0.0786921 mm / 0.0000183805 rad |
| 2.0 | 143,796 | 13,025 | 11.040x | 47,100 | 0.0783885 mm / 0.0000189663 rad |

Combined: 287,592 source bytes to 25,076 cache bytes, **11.469x**. The original
first clip used 45,622 cache bytes (3.152x). Measured scale and scalar errors are
zero in both clips. Worst conservative bounds across both clips: translation
**0.0787544 mm**, rotation **0.000348521 rad**, scale **0**, scalar
**0.000061520**. Cooking rejects measured or conservative errors over budget;
checks cover both scalar decoding and the production SIMD path. Lossless integer
prediction restores the exact quantized keys and adds no error. Bounds are local
component bounds; accumulated end-effector errors require asset-specific checks.

Resident sizes include the retained cache blob, compressed time-major packets,
root/curve metadata and constant pose, excluding allocator bookkeeping, shared
skeleton and per-actor scratch. Serialized compression and resident compression
are distinct; no expanded float key arrays are retained.

Ozz archives are **73,029 bytes** per clip and its `Animation::size()` reports
**73,250 resident bytes**, excluding skeleton and actor scratch. They exclude
Ara's curve/notifies. Measured source-relative ozz errors (same keys and 16
subdivisions): translation **0.227869 / 0.455737 mm**, rotation
**0.000115136 / 0.000165851 rad**, scale **0**. Its default translation encoding
exceeds Ara's 0.1 mm budget. The timing comparison therefore reports each
codec's actual precision, rather than claiming equal error. This is a same-assets
comparison with the default ozz builder, not an ozz compression-tuning study.
`cargo run -p ara-anim --release --example compression` reproduces Ara's inventory.

### Caller scheduling

A sixteen-actor grain produces 63 contiguous chunks for 1,000 actors. Actor
headers are cache-line aligned in the harness, with separate actor-owned output
buffers. One dispatch covers a complete actor update; the animation library has
no scheduler, locks or worker-count-dependent simulation arithmetic.
The same candidate binary was also run with these interleaved grains:

| Workers | Chunk 8 | Chunk 16 | Chunk 32 |
| --- | --- | --- | --- |
| 8 | 2.084/3.634 (60-77%) | 1.961/3.342 (56-69%) | 2.163/4.464 (52-72%) |
| 16 | 2.556/6.044 (60-71%) | 1.552/3.860 (46-72%) | 1.767/3.230 (52-66%) |

Chunk 16 had the lowest median p50 at both worker counts in this loaded kernel
calibration; chunk 32 had the lower sixteen-worker p95. The caller can choose a
grain appropriate to its workload. All calibration timings retain their host
load; they do not establish an isolated hardware optimum.

## Validation

`cargo test -p ara-anim`, `cargo clippy -p ara-anim --all-targets -- -D warnings`,
`cargo check -p ara-anim`, `cargo fmt -p ara-anim -- --check`.
All 25 tests pass. Tests cover reference accuracy, measured/certified compression
error, segmented/sub-byte/raw/predicted formats, packet boundaries, malformed
cache rejection, prefix LOD, clip rebinding, sparse/step/half-turn channels, masked/additive blends, IK,
palettes, transition conditions and trigger consumption, sync groups, multi-loop
root motion, equal-time/loop-boundary notifications, shared-DAG event weights,
canonical schema documents, typed undo and exact rollback replay. A standalone
allocator-instrumented test observes **zero allocations, reallocations or frees**
across playback, cross-fades, snapshot reuse, restore, palette generation, prefix
LOD and compatible clip rebinding.
