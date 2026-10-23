# ara-acoustics

Physical game acoustics on top of `ara-audio`: occlusion and transmission, distance
curves, air absorption, Doppler, reverb zones and portals, the listener's room reverb
(geometric early reflections + FDN) and per-cell baked reverb (Steam Audio or built-in).
Everything is deterministic for golden captures; only `RoomReverb` runs on the audio thread.

## Public API

- `Acoustics`: per-update pass. `set_zones(zones, portals)`, `probes_mut()` (cooked
  cells), `evaluate(&mut dyn RayScene, &AudioListener, &[Emitter], &mut Vec<Evaluation>)`
  → `VoiceParameters` + `capture::OcclusionRecord` per emitter (one batched trace),
  `room(&listener)` → `ReverbParams` for the room reverb.
- Components (`register(db)`): `AcousticSource` (occlusion radius, air, Doppler
  strength, reverb send, custom distance curve), `AcousticSurface` (material preset or
  custom), `ReverbZone` (box/sphere, priority, fade, material, rt60 override, wet,
  early reflections, use_baked), `AcousticPortal` (zone names `a`/`b`, "" = outdoors,
  size, `open`). Value types: `AcousticsSettings`, `AcousticMaterial`.
- `geometry::RayScene` (batch `trace(&[Segment], &mut RayHits)` + `label`), `MeshScene`
  (two-sided triangles, median BVH, `add_box/add_mesh/closest`), `physics::PhysicsScene`
  (feature `physics`, over `PhysicsBackend::queries`, `SurfaceLookup` maps user data).
- `occlusion::OcclusionSolver`: center + Fibonacci rays within the source radius;
  visibility = unblocked fraction; transmission multiplies each distinct blocker once.
- `room::ZoneSet`: `zone_at`, `listener` (boundary crossfade), `route` (shortest open
  portal path ≤ `max_portal_depth`, per-band openness/aperture/diffraction gains),
  `params` (Eyring RT60, 16π(1−α)/(Sα) level, 18 image-source early taps).
- `reverb::{room_reverb, RoomReverb, RoomReverbHandle, ReverbParams}`: aux effect for
  `AudioEngine::add_aux`; handle converts physical params to sample targets on the
  control path and pushes them through a bounded ring. Room size changes crossfade to the
  second 8-line network (0.25 s); RT60/level changes retarget in place.
- `bake::{GeometricBaker, SteamBaker, ReverbBaker, floor_probes, AcousticCell, CellKey,
  ProbeField}`: probes per `ara-world` cell (same x/z/slot key), cooked into a
  checksummed little-endian blob (`ARAACU01` + BLAKE3); `ProbeField::sample` blends the
  4 nearest probes (inverse-square) that pass a filter (same zone).

## Runtime integration (ara-runtime owner)

1. Engine setup: `let (reverb, handle) = room_reverb(rate, &settings);`
   `let room = engine.add_aux("room", Box::new(reverb))?; engine.route_aux(sfx, Some(room))?`.
2. Each presentation update (worker, not the audio thread): rebuild `ZoneSet` only when
   zone/portal components change; call `evaluate` with all audible emitters and a
   `PhysicsScene`; send each `Evaluation::parameters` as `VoiceCommand::Set { ramp_frames: 256 }`;
   `handle.set(&acoustics.room(&listener))`.
3. Streaming: decode `AcousticCell` on the world worker; `probes_mut().load/unload`.
4. Diagnostics: fill `ara_audio::diagnose::Audibility` from `Evaluation`
   (`record.blockers` → occluders) and emit `explain()` on the bus.

## Semantics worth knowing

- `reverb_send` compensates the voice's distance attenuation (≤ 16×): the diffuse field is
  distance independent. Sources in other zones send only what reaches the listener's space.
- Portal paths combine with the (occluded) direct path in energy; when the portal path
  dominates, direction and air absorption use the apparent position at path length.
- Steam's baked tails model little air loss in the top band; low/mid track Eyring.
  Long hard rooms need ~100+ bounces (`SteamBakeSettings::default` uses 128).
- The FDN quantizes room size to 15% steps so probe interpolation doesn't respawn networks.
- Each FDN line's 3-band absorption filter is scaled on the control path so its peak gain
  never exceeds the largest band gain (complementary crossover bands overshoot ~15%);
  otherwise extreme RT60 mixes (e.g. `[0, 3, 3]`) would grow without bound.
- Each network carries its own smoothed output (level × wet). A new room size switches to
  the idle network once it is quiet (a still-ringing one is faded over 10 ms first), so
  leaving a hall keeps its tail.

## Tests

`CARGO_BUILD_JOBS=2`:

```
cargo test -p ara-acoustics
cargo clippy -p ara-acoustics --all-targets --features physics,ecs -- -D warnings
cargo fmt -p ara-acoustics --check
cargo test -p ara-acoustics --test acoustics steam -- --ignored   # needs Steam runtime
```

`tests/render.rs` checks FDN RT60 calibration (Schroeder T30 within 20%), zero
allocations while rendering and retargeting, and the two-room golden capture
(`tests/golden/two_rooms.json`: WAV BLAKE3 + meters + occlusion trace). Goldens are
same-platform evidence; refresh with `ARA_UPDATE_GOLDENS=1` only for intended changes.
Steam runtime path: `ARA_STEAM_AUDIO`, else `ara-audio/runtime/steam/.../phonon.dll`.
Release bench (budget 1 ms per 128-frame block for 1,000 occluded voices with reverb sends):
`cargo run -p ara-acoustics --release --example acoustics_bench` writes `evidence/benchmark.json`.

## Gotchas

- Never trace rays, rebuild zones or decode cells on the audio thread.
- Windows reserves file names like `aux`, `con`, `nul`: never use them for modules.
- Diagnostic codes are `ara_audio::diagnose::codes` (ARA3400–3410); acoustics data
  errors use `ACOUSTICS` (3409).
