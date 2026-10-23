# ara-music

Interactive music on the `ara-audio` sample clock: segments with tempo maps and
synchronized stems, intensity layers, transitions on bar/beat/end with crossfades and
bridges, grid-aligned stingers and weighted horizontal resequencing. Every stem is a
voice scheduled at an exact output frame, so live play and offline captures agree.

## Public API

- `register(db)`: `MusicSegment` (`sg_`), `MusicTransition` (`tn_`), `MusicStinger`
  (`st_`), `MusicSettings` (`ms_`) records and the `music` kind (`.music`, `mu_` IDs).
- `SegmentDesc { name, bars, tempo: [TempoChange {bar, bpm, beatsPerBar}], stems:
  [StemDesc {name, clip, intensity: [lo, hi], gainDb}], next: [NextDesc {segment,
  weight}], loudness }`; `TransitionDesc { from ("" any), to, sync, fade, bridge }`;
  `StingerDesc { name, clip, sync, gainDb }`; `MusicSettings { initial, intensity,
  layerFade, seed }`; `MusicSync::{Bar, Beat, End, Immediate}`.
- `MusicDesc::{from_document, new, segment}`: validation (ARA3408 `MUSIC`).
- `Grid`: bar/beat ↔ frame offsets with tempo and meter changes; `next(offset, sync)`.
- `MusicPlayer::new(desc, bus, rate)`, `start`, `update` (call every control tick:
  promotes the queued segment, schedules the follow-up 0.5 s ahead), `transition(to)`
  → landing frame, `stinger(name)` → start frame, `set_intensity(x)`, `stop`,
  `position(frame)`, `history()` (scheduled segment starts).
- `StemLoader`: prepares resident or streamed voices for clip references.
- `measure_mix_loudness(&[(&Clip, gain)])`: cook-time loudness of a segment's full mix;
  store it in `SegmentDesc::loudness` so a -16 LUFS bus trims all stems equally.

## Integration (ara-runtime owner)

Own one player per music context on the audio control worker; call `update` each
control tick (it never touches the audio thread). Use a music bus with a loudness
target from the mixer document. Long stems should be streamed by the `StemLoader`.

## Tests

`CARGO_BUILD_JOBS=2`: `cargo test -p ara-music`,
`cargo clippy -p ara-music --all-targets -- -D warnings`, `cargo fmt -p ara-music --check`.
Covers grid math with meter changes, bar-exact resequencing with click onsets, seeded
order, transition/stinger landing frames, intensity layers, -16 LUFS normalization,
document round trips and a golden session (`tests/golden/session.json`;
`ARA_UPDATE_GOLDENS=1` to refresh after intended changes).

## Gotchas

- `transition` cancels a queued follow-up (stopped before it starts) and stops current
  stems at the landing frame with the rule's fade; new stems fade in over the same time.
- Default rule when none matches: bar sync, 0.5 s crossfade, no bridge.
- Voice commands must stay ordered per voice; the player tracks each stem's last command.
