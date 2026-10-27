# ara-runtime-audio

Audio in play on ara-audio v2. `AudioPresenter` owns one world's engine: the
room-reverb aux (`ara-acoustics`), the mixer (`ara-audio-mix`), acoustics against the
Jolt physics scene and the live voices. Presentation only; nothing writes simulation.
Depends on ara-audio, ara-acoustics (physics, ecs), ara-audio-mix, ara-runtime.

## Public API

- `AudioPresenter::new(&Workspace)`: 48 kHz engine, `room_reverb` aux, then
  `Mixer::build` from the first `.mixer` document (none: `master` routed to the room).
- `update(world, &listener, clips)`: reverb zones and portals take their entity's world
  pose; every `AudioSource` (with `AcousticSource`, default otherwise) is evaluated in
  one batched trace against `PhysicsWorld<JoltBackend>` (surfaces: `AcousticSurface`
  by `PhysicsId`, labels by identity); new voices start on `Mixer::bus(source.bus)`,
  existing ones get `VoiceCommand::Set` (256-frame ramp); the room reverb follows
  `Acoustics::room(listener)`. Returns `Heard` per source: distance, attenuation,
  visibility, occluders and `Audibility::explain()` diagnostics (ARA3400–3410).
- `pump(device, frames)` (live, windowed players), `capture(seconds)` (offline).
- `clip(assets, reference)`: decodes a published `ara.audio` import (resident).
- `AssetVoices` + `update_voices`: ordinary sources and music honor streamed imports
  with bounded 4096-frame queues. Live producers decode on workers; offline producers
  refill through `capture_streamed` before each render quantum. The default aggregate
  PCM queue budget is 8 MiB; compressed assets and codec packet scratch are separate.
- `MusicSource` entities play their `.music` score (one `MusicPlayer` each, following
  gameplay `intensity`/`segment`, stems prepared through `VoiceLoader`); `AudioGraphSource`
  entities play a procedural audio graph (compiled once per graph document, one
  instance per entity seeded by its identity, spatialized by its `AudioSource`,
  parameters and trigger counts forwarded at the next control frame).
- `set_music_intensity(entity, Some(value))`: presentation-only override for a
  replicated gameplay binding; `None` resumes the authored MusicSource value.
- `stream_cells(resident, assets)`: baked `AcousticCell` probes follow the world's
  resident cells (`Cell.acoustics`): decoded on a thread, installed at a later
  `update`; `wait_cells` blocks for offline captures.

## Used by

`ara-player`: windowed players open an SDL device; every presented frame runs
`stream_cells` + `update` on a scoped worker while the GPU frame presents, then `pump`
(a missing device leaves play silent); `play.captureAudio` builds a
fresh presenter, runs one update and renders offline (WAV, `<name>.audio.json`, and how
each source is heard). Hosts register `ara_audio_why` so `ara why` explains sounds.

## Gotchas

- `update` reads the simulation world only; it must not overlap a tick (the player
  runs it between ticks, beside presentation).

## Testing

`cargo test -p ara-player --test mixed -- --nocapture`: the demo campfire with an
agent-attached crackle (mixer, room reverb, ground occlusion and its ARA3402 report)
plus a VFX-composited image. `--test valley`: music, the campfire and streamed probes
in the integration level (`play.captureAudio` reports the resident acoustic cells). `cargo clippy -p ara-runtime-audio --all-targets -- -D warnings`.
