# ara-audio

Presentation audio: Kira 0.12.5 graph, SDL3 streams, deterministic offline render,
Steam Audio 4.8.1 HRTF/direct effects, bounded streaming and agent evidence.

## Public API

- `AudioEngine`: fixed 128-frame renderer, buses, effect sends, snapshots, voice
  priorities/virtualization, sample-clock commands. `render` allocates nothing.
- `PreparedVoice`: allocate/initialize on a loader, then `install` between render
  calls. Convenience `play`/`play_stream` allocate and belong on a control path.
- `Playback::from_source`, `register`: schema/ECS bridge without depending on ECS,
  simulation, asset, platform or protocol crates being developed in parallel.
  Optional `ecs` derives Bevy 0.20 Component on source/listener for direct insertion.
- `DeviceBackend`, `SdlBackend`, `OfflineBackend`, `pump`, `render_to`: shared
  renderer; sink IO and recovery run on a dedicated presentation worker.
- Windows `WasapiBackend`: IAudioClient3 RAW GameEffects, MMCSS worker, bounded
  one-period submission, default-change/loss detection and explicit recovery.
  Construct/drop on the same worker; unsupported mix formats diagnose SDL fallback.
- `SteamAudio`: explicit trusted library path; built-in measured HRTF or SOFA;
  one independent DSP state per voice. `AcousticScene` is a geometry-worker seam.
- `MediaDecoder`, `OpusDecoder`, `PackSlice`, `StreamBudget`: incremental media,
  range-confined pack reads and bounded aggregate PCM queues.
- `AudioImporter`: pure local importer contract; adapt its envelope to ara-asset
  later. Opus music/dialogue; PCM/IMA ADPCM short SFX on every platform.
- `capture_audio`: bounded offline WAV + canonical JSON, LUFS/peaks/spectrum,
  silence, clipping, pre-limiter overloads and a WAV BLAKE3 hash.

## Gates

Always set `CARGO_BUILD_JOBS=2`. `ara-opus-sys` builds committed libopus 1.5.2
with `cc`; only Rust and the C compiler are required for this track's build.
SDL is statically built and linked once through `ara-sdl3-src`, shared with
`ara-platform`. `SdlBackend::open(sample_rate)` takes no library path. Only
Steam Audio uses explicit runtime loading; `libloading` remains for that API.
The clean-environment gate strips CMake/Python from PATH:

```powershell
powershell -NoProfile -File crates/ara-audio/tools/check_clean.ps1
```

```
cargo check -p ara-audio
cargo test -p ara-audio
cargo clippy -p ara-audio -p ara-opus-sys --all-targets -- -D warnings
cargo fmt -p ara-audio -p ara-opus-sys --check
cargo check -p ara-audio --features ecs
```

Explicit native checks (no device is opened by ordinary tests):

```
powershell -NoProfile -File crates/ara-audio/tools/fetch_runtime.ps1
cargo test -p ara-audio --test native -- --ignored
cargo run -p ara-audio --release --example verify -- offline
cargo run -p ara-audio --release --example verify -- bench
cargo run -p ara-audio --release --example verify -- device
```

`tools/loopback.py` needs soundcard 0.4.6 + numpy and takes the absolute/relative
path to the compiled `verify.exe`. It records only the default output loopback.
Python is an optional measurement tool, never a Cargo build dependency.
The verifier selects WASAPI on Windows; `ARA_AUDIO_BACKEND=sdl` selects SDL.
`ARA_AUDIO_LATENCY_TRIALS=32` requests a longer QPC/MLS loopback measurement.
See README for actual measured latency versus the much smaller queue estimate.
The fetch helper is only for optional Steam Audio; SDL requires no setup.

## Gotchas

- Never load, decode, cook, create HRTFs, mutate a graph or join a worker inside
  `render`. Prepare assets out of process/on loader workers under D20.
- Streaming workers lower their Windows priority; other hosts should set their
  corresponding loader priority when integrating. Drop worker guards off audio/UI.
- Kira owns the graph/DSP lifecycle; Ara owns voice policy and the sample clock.
  Preserve this seam; Windows low latency and SDL share DeviceBackend; no cpal or global SDL_Quit.
- SDL watermark must cover a whole hardware period plus one DSP quantum.
- SDL queue latency is a lower bound, not a measured output latency guarantee.
- Zero bus/send gain uses Kira's **finite** silence value, never negative infinity.
- Goldens are same-platform/native-version evidence, not cross-platform netcode.
- Playback cursors, DSP history, decode queues and devices never enter simulation
  snapshots or authoritative C# state. Authoring edits remain ara-ops transactions.
- Offline stream goldens synchronously prebuffer; racing a worker is not deterministic.
- Preserve exact WAV fact lengths and Opus pre-skip/EOS trim. Decoder scratch is
  separate from queued-PCM/resident limits; importers run in bounded worker processes.
- Native binaries/artifacts are ignored. Preserve upstream redistribution notices
  when packaging; do not commit SDK binaries or modify root workspace manifests.
