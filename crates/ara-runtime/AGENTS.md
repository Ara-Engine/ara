# ara-runtime

Composes the merged subsystems into one playable world: `ara-sim` session + input
actions + Jolt physics + ara-anim animation + Recast navigation + cooked assets, with a
renderer-neutral presentation extraction. No GPU, window or process code (see
`ara-runtime-render`, `ara-player`).

## Public API

- `engine::register_schemas(db)` / `engine::register_commands(registry, db)`: the one
  engine schema world (kernel, runtime, physics, nav, animation documents, audio, 2D,
  2D physics, world, l10n, save, terrain, foliage, materials, VFX commands). Every host
  (CLI, daemon, editor via `ara_play_host::open_project`) and the player register
  exactly this; clashes are returned, never silently skipped.
- `schema(project_types)`: the engine schema world + project `*.types.json`.
  The engine world includes brains/perception (`Brain`, `Perceiver`, `Stimulus`) and
  the behaviour commands (`behavior.*`).
- `Runtime::new(schema, workspace, scene, assets, RuntimeConfig, plugins)`: projects the
  scene, loads every `MeshRenderer.model` generation, builds physics/nav/animation at
  tick 0 and returns **paused**. `step`, `step_until`, `advance(elapsed)` (bounded
  catch-up), `pause/resume/set_timescale`, `inject`, `patch_edits` (live edits →
  baseline-checked ops at the next tick), `changes()` (keep-changes diff),
  `extract(&mut RenderFrame)`, `reload_assets`, `fault()`, `navigation_errors()`.
  `play()/play_mut()` expose `ara_sim::PlayControl` (watch, checkpoint, record);
  restore and replay through `restore(snapshot)` / `replay(replay)` so plugins
  resynchronize (`RuntimePlugin::restored`).
- `components`: `MeshRenderer` (shape or imported model), `Light`, `Environment`,
  `Camera`, `FollowCamera`, `PlayerControl`, `Wanderer`, `Animator` (+ `LocomotionClip`),
  `VisualEffect` (a cooked GPU particle effect). Also simulated: ara-2d sprites,
  tilemaps, 2D lights and cameras, Rapier2D bodies (2D physics installs when the scene
  has 2D bodies; `PlayerControl` on a `CharacterController2d` drives it), and
  ara-audio `AudioSource`/`AudioListener` plus ara-acoustics `AcousticSource`,
  `AcousticSurface`, `ReverbZone`, `AcousticPortal` (presentation intent only; played by
  `ara-runtime-audio`). Mixer and music documents are part of the engine schema world.
  `MusicSource` (a `.music` score) and `AudioGraphSource` (a procedural audio graph)
  are presentation intent too. `StreamedWorld` names the `.world` a scene streams
  (`ara-runtime-world`); `Terrain` (+ `.foliage`) and `TerrainChunk` (collision,
  navigation and landscape pages per tile; author them in world cells to stream).
- `ai`: brains on nav v2 (`install` compiles `graph` documents of domain `behavior`
  into `AiWorld`; `adopt`, `poses`; `AiProvider` snapshots `BrainState`,
  `Perception`, `PerceptionPose` and the intents). Brains need a `NavAgent`.
- `navigation`: per-profile bakes sized by the profile's `NavAgentType` (else its
  first agent) over static boxes and terrain, registered with `set_bake_source` so
  `NavObstacle` carve/area changes rebake; streamed worlds with terrain bake every
  chunk's nav cell at load and `NavStreaming` follows the resident chunks
  (`NavStreamer` assembled at the boundary, swaps recorded with their tick).
- `assets`: `AssetSource` (published generations by `as_…` ID), `CachedAssets`
  (FileCache pinned by action digest), `MemoryAssets`, `Models`.
- Missing/broken model generations produce `asset_errors()` (`ARA7303`) and magenta
  shape placeholders. Failed IDs remain eligible for a later hot-reload recovery.
- `model::Model` decodes a glTF import generation (FORMAT.md v1); `skin::Skin` turns
  the cooked skeleton/clips into ara-anim data (parent-first reorder, `joint_map`).
- `plugin::RuntimePlugin` (`Send + Sync`): seams for C# scripts (`ara-runtime-script`),
  networked play (`ara-runtime-net`), world streaming (`ara-runtime-world`), audio and
  VFX (`prepare(world, schema)` before projection, `register`, `install(Stage)`,
  `before_tick`, `stream`, `after_tick`, `mirror`, `restored`, `frame`).
- Networked play (`ara-runtime-net`): `step_with_adapter(adapter)` runs one boundary
  for an ara-net server (plugins around `Session::step_with_adapter`; the server's
  timeline is the clock). `present_mirror(tick)` advances a never-stepped client
  mirror's presentation (animation, cameras, world poses); `tick()` then reports the
  presented authoritative tick.
- `controller`: `.animation` controllers (`Animator.controller`): `Documents` (every
  `AnimAsset` record by `@path#record`; several clips/controllers per file), `compile` (clip records bind model animations by `name`
  when they hold no keys; motion-matching nodes cook their `motionDatabase`),
  `MatchState` (trajectory predictor + feature query, predictor snapshotted).
- `extract::RenderFrame`: camera, sun, lights, environment, draws (shape or model
  primitive, interpolated world matrix) and skin-order palettes.
- `control::default_actions()`: `Move`, `Look` (Axis2), `Jump`, `Sprint` + bindings.

## System order

Documented in `lib.rs`. Boundary (inputs, patches, commands, streamed arrivals) →
input → plugin Gameplay → wander → adoption → `navigation.stream` → obstacles → AI
(`sync → perceive → think → bridge`) → navigation (exclusive) → `ai.locomotion` →
`animation ∥ navigation.follow` (parallel group) → root motion → terrain chunks /
physics adoption → physics.push → physics (exclusive) → physics.pull → camera.place →
plugin Late → poses.resolve.

## State and determinism

- Gameplay state is registered components (physics mirrors, `PlayerControl.yaw`,
  `FollowCamera.yaw/pitch`, `Wanderer` cursors). Provider-owned state:
  `physics` (native world; exact in-process checkpoints, rebuilt from mirrors on a
  portable restore), `navigation` (`NavPosition`, `NavAgentState`, obstacle
  positions, pending obstacle rebakes, streamed resident cells), `ai` (brains,
  perception, intents), `animation` (`AnimatorSlot` snapshots), `poses` (derived
  world/previous/synced poses), `terrain` (immutable evaluated tiles).
- Same seed + inputs → same state hash; tests check two runtimes, exact checkpoint
  rollback and portable tick-0 snapshot restore.
- Presentation (`extract`) only reads; it interpolates previous→current world poses.

## Conventions and gotchas

- `Transform.rotation` is intrinsic XYZ Euler degrees (`glam::EulerRot::XYZ`); see
  `math.rs`. Cameras/lights look along local -Z; characters face +Z at yaw 0.
- Physics: a body is teleported only when its `Transform` differs from what physics
  last wrote (`SyncedTransform`). Static bodies keep authored transforms. Kinematic
  nav agents lift their collider so it rests on the ground.
- Navmeshes bake at load from static box colliders and terrain inside each
  `NavMeshSurface` (cached by tile content in `nav_cache`). `NavMeshSurface.geometry`
  assets are not baked yet (structured error). Streamed navigation needs terrain
  tiles that are whole nav tiles on the same grid; streamed cells' static colliders
  do not carve it (use `NavObstacle` with `Avoid`), and carve/area obstacles only
  rebake static (non-streamed) meshes.
- Terrain: one `Terrain` per level; tiles evaluate on worker threads at load (strokes
  only; imported bases are not available in play yet). Chunk bodies are cooked Jolt
  heightfields, re-cooked after every native rebuild.
  The Terrain entity's complete parent-resolved translation shifts the evaluated
  grid, heights and foliage after stroke/exclusion evaluation; navigation and
  collision consume that same world data. Rotation/scale must be baked into the
  grid and diagnose if present. Chunk geometry already contains the world offset.
- Animation: tick rate must divide 48 000. The move-state cycle is the mean of the
  locomotion clip lengths. Cubic glTF channels keep key values; morph channels are
  not animated. A reloaded model restarts its animators (old snapshots are rejected).
- Models: glTF imports only; FBX manifests return a structured error.
- Live patches always apply at the next tick boundary (paused worlds reach it by
  stepping). `changes()` compares at f32 precision with defaults materialized and
  marks system-written values `driven`.

## Testing

`CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p ara-runtime`. `tests/demo.rs`
cooks the Fox in-process from `../ara-player/demo` and plays the demo scene: walking
and grounding, crowd roaming, skin palettes, determinism, exact/portable restore, live
patch + keep changes, the canonical `anim/fox.animation` (authored by typed commands;
`ARA_UPDATE_DEMO=1` rewrites it) and a motion-matching Fox (deterministic, predictor
survives checkpoints). `cargo clippy -p ara-runtime --all-targets -- -D warnings`.
