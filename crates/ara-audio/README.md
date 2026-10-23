# Ara audio runtime

`ara-audio` owns presentation audio. No dependency on the in-flight platform,
simulation, asset or protocol crates is required. All source/listener properties
and command parameters have schema documentation. Gameplay state remains in ECS;
playback cursors and DSP history are local presentation state.

## Mixer choice

Use **Kira 0.12.5 with an Ara-owned pull backend**, following A11. Its renderer can
be owned directly, with no mutex, cpal dependency, device callback or wall clock.
Its existing tree/send graph, prepared effect buffers, control/resource queues,
smoothed parameters and control-side resource reclamation remove substantial DSP
and lifecycle code from Ara. Custom `Sound` and `Effect` boundaries let Ara own
the behavior that differs: priorities, virtual voices, exact sample scheduling,
Steam Audio, ducking and overload-aware capture.

An Ara-owned Firewheel-class graph would give arbitrary graph topology and allow
node fusion or parallel rendering. It would also require us to own graph
compilation, cycle rules, cross-thread publication, resource retirement, effects,
sends, automation and tests. Neither better HRTF quality nor lower hardware latency
follows from owning that graph: HRTF work is in Steam Audio and device latency is
in SDL/Windows. Existing requirements fit Kira's submix/send graph and extension
points. The measurement below supports keeping this seam. We did not build a
second equivalent graph or claim a head-to-head performance win against Firewheel.

Primary APIs reviewed: [Kira backend/renderer](https://docs.rs/kira/0.12.5/kira/backend/index.html),
[Kira Sound](https://docs.rs/kira/0.12.5/kira/sound/trait.Sound.html),
[Firewheel graph](https://github.com/BillyDM/firewheel),
[Steam Audio C API](https://valvesoftware.github.io/steam-audio/doc/capi/).

## Integration

1. Register `ara_audio::register(&mut SchemaDb)` alongside other engine components.
   The C# generator/inspector/protocol use these same descriptors. `AudioSource`
   and `AudioListener` are plain schema components by default. The optional `ecs`
   feature also derives Bevy 0.20 `Component`, so simulation can insert these types
   directly without duplicating them or depending on a simulation crate.
2. Authoring updates remain typed ara-cmd → ara-ops transactions. The ECS bridge
   reads resolved source/listener/world-transform values and builds
   `Playback::from_source` or `VoiceCommand::Set`. These commands affect only audio.
   Register `PlayAudioParams`, `ControlAudioParams`, `StopAudioParams` and
   `CaptureAudioParams` in the protocol's runtime/job
   dispatch; `VoiceId` is session-local, never an asset or network ID.
3. On a loader worker prepare the clip/stream, optional Steam effect and
   `PreparedVoice`. Move it through a bounded queue to the dedicated audio worker;
   `AudioEngine::install` aligns its clock at a render boundary. Graph edits,
   resource reclamation and worker joins are control operations. Keep the current
   graph playing while agents/importers prepare the next version; respect the
   host's immediate/idle/approval and active-Play application policy (D20).
4. Resolve source bus names. `add_bus` builds an acyclic hierarchy. Effects are
   ordered effect builders: Kira EQ, HP/LP, compressor and algorithmic reverb,
   plus an Ara stereo delay whose first echo is independent of feedback;
   `PeakLimiter` is a stereo-linked, zero-lookahead safety stage. `snapshot` captures
   bus targets; `apply_snapshot` validates atomically and smooths them. `Ducking`
   uses bus meters, attack/release, and at most one quantum of sidechain delay.
5. Shared room effects use `add_send(SendTrackBuilder...)`,
   `TrackBuilder::with_send` and `set_send`. Apply an acoustic worker's
   `AcousticPath::reverb_send` to the appropriate room/source-group bus route.
6. Use the **same trusted SDL3 library as ara-platform**. sdl3-sys supplies ABI
   types with `no-link`; we load the existing runtime rather than building another
   SDL. One audio subsystem reference is balanced on drop; process-wide SDL_Quit
   is never called. Default streams migrate automatically; format/loss polling
   never consumes platform events. The host can request `recover` after loss;
   reopening a stream preserves the engine/voices. Call `pump` frequently on the
   audio worker, with a watermark covering the hardware period plus 128 frames.
7. Short sounds use resident `Clip`; long audio uses `PackSlice`, a decoder worker
   and an SPSC PCM queue. `StreamBudget` charges all live queues until both ends
   drop; resident decode has an explicit byte limit. Codec scratch and process
   memory remain separate worker limits. Starvation emits silence and counts
   underruns. Worker cancellation/teardown happens off the audio/UI threads.
8. Adapt the local `Importer` envelope to ara-asset's eventual worker contract.
   Music/dialogue cook to 48-kHz Ogg Opus; short SFX to PCM16 or IMA ADPCM WAV.
   Mobile changes the quality preset, not codecs. Cooking is pure, includes a
   stable importer version and content hash, and performs no document/file writes.
9. Feed presentation events from simulation at the chosen tick/interpolation
   boundary. Deduplicate predicted/replayed event IDs in the bridge; never rewind
   devices, decoder queues or sound cursors as part of netcode rollback.
10. Run `capture_audio(seconds)` in an isolated presentation/replay engine. It
    renders the same fixed quanta as SDL, with no worker/device timing dependency.
    WAV/JSON jobs may run on agents without pausing or replacing human playback.

The parent integration should refresh the root Cargo.lock once for the merged
tracks; this track owns and commits only this crate.

## Spatial status

Implemented: listener-local 3D equal-power panning, inverse/linear/inverse-square
range curves, source and listener cones, bounded Doppler, cubic runtime resampling,
gain/rate smoothing, and click-reduced fallback panning/occlusion. Non-spatial
music/UI preserves stereo. Priorities choose audible voices once per quantum;
virtual voices continue source time. Gain and rate commands start on an exact
sample; geometric updates apply at the next quantum. Capacity, stale/past handles,
command ordering and queue overflow are explicit errors.

**Steam Audio is working**, not a stub: an owned thin 4.8.1 C binding loads the
built-in measured HRTF or a SOFA file, creates per-voice bilinear binaural and
three-band occlusion/transmission effects, and drains native convolution tails.
Context/HRTF handles are cloned only during preparation and read immutably during
rendering; each DSP instance has one owner.
The SDK allocator is routed through Rust, so the render allocation probe includes
its allocator callbacks. Native Windows goldens test direction, silence under full
occlusion, and repeatable output.

**Geometry simulation and baked reflections are a worker integration seam**
(`AcousticScene` → `AcousticPath`), not an implemented scene tracer/baker.
Room reverb currently uses Kira's algorithmic send. This is the permitted spatial
seam for this pass; Steam's measured dataset is the real HRTF path. No claim is
made that reflection geometry has been integrated or that a synthetic panner is
an HRTF. SDK archives are pinned by SHA-256 in `tools/fetch_runtime.py`, which
extracts Windows/Linux/macOS/Android/iOS native artifacts. SDL packaging outside
Windows belongs to ara-platform. Only Windows was executed here; iOS static
linking and Android library packaging still need the export/platform adapters.

## Verification and measurements

See `AGENTS.md` for the exact gates and explicit device/benchmark commands.
`examples/verify.rs` writes canonical JSON and WAV to ignored `artifacts/`.
`evidence/` holds the recorded results for this implementation. Neither binaries
nor downloaded SDKs are committed. Offline capture reports EBU R128 integrated
LUFS (null below 400 ms/silence), sample/true peak, RMS, spectrum bands, silence,
clipping and exact delivered pre-limiter overload counts, plus a PCM16 WAV hash.
The normal tests open no physical device; the explicit native test uses the pinned
SDK. Goldens are same-platform evidence, not cross-platform lockstep promises.

Measurements below were recorded on the Windows 11 Ryzen 7 7800X3D machine, Rust
1.99.1, release profile, two Cargo jobs, 48 kHz, 128-frame quantum (2.667 ms).
They include control-queue consumption, resampling, spatialization, Kira mixing,
meters and final limiting, with 32 warmup and 256 measured quanta. Scheduler and
other parallel engine tracks contribute timing variance. They are not a universal
device-performance guarantee. Exact final values are in the evidence JSON.

- Baseline before the command/position hot-path change: 1,000 audible stereo
  spatial voices averaged **1.4195 ms**; full bilinear HRTF averaged **6.8018 ms**.
- Final means: **1.4359 ms** for 1,000 audible stereo spatial voices,
  **8.0541 ms** for 1,000 full-HRTF voices, and **1.7584 ms** for 128 HRTF + 872
  virtual voices (budgeted p95 **2.0477 ms**). Timing variance is substantial under
  concurrent engine-track builds; these are observations, not an isolated causal
  speedup claim. Final CPU, allocation, latency, device and spatial measurements are
  recorded in `evidence/benchmark.json`, `loopback.json`, `device.json` and
  `spatial.json`. Full 1,000-voice HRTF exceeds the 2.667-ms deadline. The default
  audible budget is 128; remaining voices advance virtually, skipping native DSP.
  The budgeted case is measured as well; scheduler outliers still occur.
- Every measured render quantum had **zero allocations and zero deallocations**
  through Rust and the supplied Steam allocation callbacks, including a separate
  prepared-install/first-render/fade/stop/tail probe. SDL/OS C allocator
  internals are outside that probe. Device submission/recovery is not the render
  callback and may allocate.
- Real output: **Speakers (BlackShark V3 - Game)**, 48 kHz, 480-frame hardware
  period. Playback was independently captured through output-only WASAPI loopback;
  explicit stream destruction/reopen also succeeded with ongoing voices.
- Eight MLS bursts measured **82.51 / 83.28 ms p50/p95 command → Windows output mix**, with
  correlation essentially 1.0. The queue + SDL hardware-buffer estimate is only
  **22.67 ms** and misses hidden driver/SDL buffering. The loopback probe uses
  WASAPI packet QPC timestamps, not Python record-call timing. It excludes DAC,
  wireless transport and acoustic propagation. Sample-frame and raw/game-role
  hints did not materially improve this device's result; low-latency platform
  tuning remains an integration target, not a solved claim.
- The spatial door demo is a two-second left/right render with measured HRTFs:
  **-19.03 LUFS**, **0.898 sample/true peak**, no clipping. Its spectrum and WAV
  hash are preserved in the evidence. All **26 tests**, including the explicit
  native HRTF golden, passed; check, clippy with denied warnings and fmt passed.

## Native and fixture provenance

Steam Audio's API/runtime is Apache-2.0; the binary SDK includes third-party
notices (including its HRTF/FFT dependencies) that must ship with an export.
The downloader preserves `steamaudio/THIRDPARTY.md` and API headers. See
[Steam Audio source/license](https://github.com/ValveSoftware/steam-audio/tree/v4.8.1).
SDL3 is zlib; its runtime archive's LICENSE.txt is preserved. Rust dependencies
remain Cargo dependencies, not copied source. Symphonia/Opus codec support uses
their upstream implementations; no Flax/Unity/Unreal source was transcribed.

`tests/fixtures/tone.flac` and `tone.ogg` are Ara-generated 2,400-frame procedural
stereo sawtooths (no external recording), encoded with ffmpeg FLAC/libvorbis and
metadata removed. They verify real incremental FLAC/Vorbis decoding. Goldens
use a fixed integer LCG, fixed scheduling and reviewed WAV BLAKE3 constants.
