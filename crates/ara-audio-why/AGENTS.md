# ara-audio-why

`ara why` explainer for audio: "why can't I hear X?". Pure logic over `ara_why::Facts`;
depends only on ara-why and ara-diag so the project host can register it without
linking the audio engine.

## Public API

- `register(&mut ara_why::Why)` adds `AudioExplainer` (name `audio`). The host owner
  calls `ara_audio_why::register(host.why())` next to the built-ins.
- Matches questions with an entity (`e_8b21`, `name:Radio`) or a capitalized name plus
  audio words (hear, sound, silent, quiet, mute, ...); visibility questions stay with
  the visibility explainer.
- Document checks: `AudioSource` present, clip resolves (fix: closest asset), gain > 0,
  distance from the scene's `AudioListener` (world transforms) vs `maxDistance` (fix:
  `property.set` to 1.25× the distance).
- Runtime causes come from the diagnostics bus: the audio runtime emits
  `ara_audio::diagnose::Audibility::explain()` (ARA3400–3410) per entity; the newest
  report per code becomes a cause, with its fixes carried over.

## Integration

The runtime should emit audibility reports for sources that are expected to be heard
but are not (or on request from tools), keyed by `Subject::entity`. Without reports the
`runtime` check is `?` and the answer says to play the scene.

## Gotchas

- Codes resolve by catalog name (`audio::OCCLUDED`) from `ara-diag/codes/subsystems.md`;
  `tests/explain.rs` asserts they equal `ara_audio::diagnose::codes`.

## Tests

`CARGO_BUILD_JOBS=2`: `cargo test -p ara-audio-why`,
`cargo clippy -p ara-audio-why --all-targets -- -D warnings`, `cargo fmt -p ara-audio-why --check`.
