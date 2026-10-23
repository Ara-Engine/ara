# ara-physics

Ara's physics vocabulary, reflected components, fixed-tick contract and Bevy ECS 0.20 seam.
Contains no native code or Jolt dependency. `ara-physics-jolt` implements its backend.

## Public API

- `register_schemas(&mut SchemaDb)` registers authoring, input and mirrored-state types.
- `PhysicsBackend`: batch body commands/extraction, shapes, six joints, narrowphase
  queries, CharacterVirtual-style movement, wheeled vehicles, snapshots and debug data.
- `BodyColumns`: reusable SoA position/rotation/velocity/awake columns for ECS.
  The native backend fills these without an intermediate BodyState conversion.
- `PhysicsWorld<B>`: immutable fixed interval, tick number, events and native checkpoints.
- `register::<B>(&mut World, &mut Schedule, config)` installs the `PhysicsSet` boundary.
- `PhysicsId` is nonzero, unique and stable across peers. Creation/input ordering uses
  it instead of Bevy entity IDs or table order. Backend handles never enter documents.
- `PhysicsCommands` queues typed runtime intents by stable identity. These are gameplay
  intents, not authoring edits. Authoring still goes through ara-ops transactions.
- `snapshot_ecs::<B>` / `restore_ecs::<B>` are trusted checkpoint companions.
- `body_for_id` / `id_for_body` resolve transient bodies and stable identities.
- Contact effects retain stable application identities after bodies are removed.
- `PhysicsStatus` holds a sticky schedule fault; the host pauses only that session.

## ara-sim integration

1. Register every reflected component with `SimRegistry::register_component`:
   PhysicsId, PhysicsPose, RigidBody, Collider, Velocity, CharacterController,
   CharacterInput, CharacterMotion, Vehicle, VehicleInput, VehicleTelemetry and Joint.
2. Run gameplay input systems before `PhysicsSet` in the fixed schedule. Resolve parent
   transforms and bake scale into geometry first. `PhysicsPose` is world-space quaternion
   data; ara-components' authoring Transform currently uses parent-relative Euler degrees.
   The host clears Bevy change/removal trackers at its frame boundary after all
   consumers finish; otherwise resource-scope removal history grows over time.
3. `prepare_ecs` builds native objects without advancing time; `snapshot_ecs` also does this
   for an initial tick-zero checkpoint. Retain `EcsPhysicsSnapshot<B::Snapshot>` beside
   the ECS/RNG/input checkpoint at the same
   completed tick. Restore the simulation's other state, then `restore_ecs` before replay.
4. Treat PhysicsWorld, the private integration cache, PhysicsCommands and PhysicsStatus as
   external infrastructure owned by this checkpoint provider. Register an ara-sim
   StateProvider and exclude these resources from its strict ECS snapshot;
   serializing only the registered ECS values is insufficient for native rollback.
   `infrastructure_type_ids::<B>()` exposes all four resource TypeIds to that hook.
5. Restore requires identical native structure. For lifecycle changes, reconstruct the
   checkpoint's world in stable-ID order, then replay lifecycle commands and inputs.
   The adapter rejects mismatched topology before mutating native state.

## ara-render / ara-asset integration

- `DebugPrimitive` is a typed presentation stream. Cache `DebugGeometry` by shape handle;
  compose compound child transforms recursively. No renderer crate is linked here.
- `Shape::ConvexHull` / `Shape::Mesh` accept raw positions/indices. The asset worker does
  cooking outside the editor tick (D20). Once ara-asset-format is available, expose its
  vertex/index slices to this API and resolve Collider.asset to a `CookedCollider`.
- Install the last good cooked shape only at the host's allowed tick boundary. A cooked
  handle belongs to one world; re-cook/re-resolve assets when reconstructing a world.
- `Feature::Physics2d` reserves capability negotiation for M4. A 2D backend gets its own
  dimension-specific geometry, never a reduced 3D vocabulary.

## Rules and validation

- All reflected fields need documentation, units/ranges where meaningful, and defaults
  identical to Rust/Serde. Tests validate schema metadata and every default.
- Native bodies/characters are mirrored into ECS on every completed boundary; equal
  values retain Bevy change ticks, and missing mirrored columns are reinstalled. Effects
  carry tick IDs; rollback consumers replace/reconcile effects for replayed ticks.
- Query results are precise and sorted; never use unordered broadphase results in gameplay.
- 15 velocity / 2 position iterations are the production defaults (S6 tower requirement).
- `PhysicsConfig::{game_default,stress,deterministic_network}` select documented
  solver/memory/effect policies; see ara-physics-jolt/benches/RESULTS.md. Default
  still reproduces S6. Sleeping/speculative distances and island splitting are explicit.
- ECS lifecycle batches preserve PhysicsId construction order across actor kinds.
  A failed batch discards unfinished bindings so correcting a scene permits retry.
  Stable identity sets use linear configuration/joint checks; identity or collider
  changes retain the full structural synchronization and broadphase optimization.
- The `bench` feature adds PhysicsTimings for lifecycle, inputs, step, extraction
  and writeback. Wall-clock diagnostics never exist in ordinary simulation builds.
- `cargo test -p ara-physics`; behavior tests are in `ara-physics-jolt`.
- `cargo clippy -p ara-physics --all-targets -- -D warnings`.
- `cargo fmt -p ara-physics -- --check`.
