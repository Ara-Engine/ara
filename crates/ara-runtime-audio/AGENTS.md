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

## Used by

`ara-player`: windowed players open an SDL device and call `update` + `pump` every
presented frame (a missing device leaves play silent); `play.captureAudio` builds a
fresh presenter, runs one update and renders offline (WAV, `<name>.audio.json`, and how
each source is heard). Hosts register `ara_audio_why` so `ara why` explains sounds.

## Not yet

- Music (`MusicPlayer::update`) and procedural `.audiograph` voices
  (`PreparedVoice::procedural`): no scene component names which entity plays them yet.
- Baked `AcousticCell` streaming with `ara-world` cells (no runtime streaming boundary).
- Evaluation runs on the player's frame thread, not a dedicated audio-control worker.

## Testing

`cargo test -p ara-player --test mixed -- --nocapture`: the demo campfire with an
agent-attached crackle (mixer, room reverb, ground occlusion and its ARA3402 report)
plus a VFX-composited image. `cargo clippy -p ara-runtime-audio --all-targets -- -D warnings`.
