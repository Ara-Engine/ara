# ara-audio-mix

Mixer documents for `ara-audio`: the bus (category) tree, base gains and mutes,
room-reverb routing, sidechain ducking, per-category real-voice limits, loudness
targets and named mix states (combat, explore) with transition times.

## Public API

- `register(db)`: records `AudioBus` (`b_`), `AudioMixState` (`s_`),
  `AudioMixerSettings` (`m_`) and the `mixer` document kind (`.mixer`, `mx_` IDs).
  Edits go through the generic record/property commands; no side-channel writes.
- `BusDesc { name, output, gainDb, mute, voiceLimit, loudness, reverb, duck }`;
  `DuckDesc { by, thresholdDb, amountDb, attack, release }`;
  `MixStateDesc { name, buses: {bus: dB offset}, fade }`; `MixerSettings { initialState }`.
- `MixerDesc::{from_document, new}`: validates names, outputs, duck sources, cycles,
  numbers and state targets (ARA3407 `MIXER`); orders buses so outputs and duck
  sources are created first.
- `Mixer::build(&mut AudioEngine, desc, room: Option<AuxId>)`: creates buses
  (ducking effects on the bus builder), applies voice limits, loudness targets
  (music/dialogue: -16 LUFS) and reverb routes, then the initial state.
  `Mixer::{bus, state, gains, set_state(engine, Some("combat"), fade_override)}`.
- `linear(db)`: dB to linear, -96 dB and below is silence.

## Integration (ara-runtime owner)

Build after creating the room reverb aux (`ara-acoustics`), before playback. Resolve
`AudioSource.bus` with `Mixer::bus`. Gameplay changes state through `set_state` on the
audio control path (a presentation intent, not an authoring edit).

## Gotchas

- The kernel reserves record fields named `parent`; buses use `output`.
- Records decode with `#[serde(default)]` because the canonical writer omits defaults.
- Ducking reads the source bus meter's block peak: one quantum (2.7 ms) of sidechain latency.
- `master` may be configured by a bus record named `master` with an empty output.

## Tests

`CARGO_BUILD_JOBS=2`: `cargo test -p ara-audio-mix`,
`cargo clippy -p ara-audio-mix --all-targets -- -D warnings`, `cargo fmt -p ara-audio-mix --check`.
Covers schema docs/defaults, canonical document round trip, validation messages,
dialogue ducking music by more than 6 dB (bus meters), category limits and states.
