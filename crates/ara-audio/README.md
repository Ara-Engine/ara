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

## Compiler-only native build

`ara-opus-sys` replaces opus/opusic-sys. It commits the portable floating-point
libopus **1.5.2** C sources and compiles them with `cc`, following S6's owned-shim
pattern. Two fixed-signature C wrappers cover the variadic bitrate/lookahead
controls. Its README records the release archive hash and retained BSD notices.
Cargo uses no CMake, Python, generated bindings, assembler or native downloads.
Opus allocations remain on importer/decoder workers, outside render. Importer
version 2 invalidates v1 cache entries because the codec pin/configuration changed.

`sdl3-sys` has default features disabled and `no-link`; it builds no SDL and
downloads nothing. SDL/Steam DLLs are loaded only from an explicit trusted runtime
path. Windows WASAPI calls system DLLs via generated Rust bindings. Neither SDL
nor Steam Audio is needed for `cargo check --workspace` or normal audio tests.
`tools/check_clean.ps1` strips CMake/Python from PATH and runs both gates in a
separate target directory. The initial fresh build reproduced the CMake failure;
the same target then compiled the replacement using MSVC alone.

Optional setup uses `tools/fetch_runtime.ps1`, which verifies pinned archive hashes
and preserves licenses/headers. `tools/loopback.py` is an optional measurement
tool, never a build dependency.
The root workspace manifest is unchanged; Cargo.lock records the new dependencies.

## Windows low-latency output

Choose `wasapi::WasapiBackend::open(rate, WasapiOptions::default())` on the audio
worker for Windows 10+. It uses IAudioClient3 shared RAW GameEffects and MMCSS Pro
Audio, with one negotiated period queued. No extra pull ring is inserted. The
128-frame mixer cache is unchanged. The backend is thread-affine and balances
COM/MMCSS teardown after its interfaces. It detects client loss/default changes;
`recover` reopens output while the engine retains its presentation voices.
`pump` rejects watermarks larger than native capacity before advancing the mixer.

The fast path requires a matching stereo f32 endpoint mix; unsupported formats,
RAW or IAudioClient3 return diagnostics. The platform adapter can select SDL for
conversion or other platforms. SDL still requests 128 frames and reports the
negotiated result. This keeps A9's SDL platform layer and A11's Kira graph; the
explicit Windows audio specialization follows this performance pass's brief.
The verifier uses WASAPI on Windows; set `ARA_AUDIO_BACKEND=sdl` for comparison.
No user/system audio settings or exclusive-mode ownership are changed.

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
an HRTF. SDK archives are pinned by SHA-256 in `tools/fetch_runtime.ps1`, which
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

- Original means: **1.4359 ms** for 1,000 panned voices and **1.7584 ms** for
  128 HRTF + 872 virtual. This pass's fresh baseline was **1.4714 / 1.9157 ms**.
- Final means: **0.3659 / 0.5149 ms**, with p95 **0.4361 / 0.6895 ms**.
  Unit-rate resident blocks skip cubic interpolation and constant smoothing,
  retain identical look-ahead history, and use independent block multiply loops
  amenable to LLVM vectorization. Virtual resident voices advance in constant
  work; priority membership uses linear selection instead of sorting every voice.
  Loop seams, short loops, promotion and later rate changes compare exactly with
  the sample path. The original offline and native HRTF goldens are unchanged.
  Full 1,000-voice bilinear HRTF still averages **4.6731 ms**, beyond the deadline;
  the default real-voice budget remains 128. No HRTF interpolation quality was cut.
  These measurements include scheduler variance on a shared development PC.
- Every measured render quantum had **zero allocations and zero deallocations**
  through Rust and the supplied Steam allocation callbacks, including a separate
  prepared-install/first-render/fade/stop/tail probe. SDL/OS C allocator
  internals are outside that probe. Device submission/recovery is not the render
  callback and may allocate.
- Real output: **Speakers (BlackShark V3 - Game)**, 48 kHz, 480-frame hardware
  period. Playback was independently captured through output-only WASAPI loopback;
  explicit stream destruction/reopen also succeeded with ongoing voices.
- Original loopback was **82.51 / 83.28 ms p50/p95**; this pass's fresh baseline
  was **92.57 / 93.67 ms**. SDL's 128-frame hint still negotiated 480 frames;
  RAW alone through SDL did not improve it. Its pinned WASAPI source disables
  stream-role selection. Direct ordinary shared IAudioClient3 measured
  **70.05 / 71.45 ms**. RAW GameEffects on that same endpoint measured
  **25.28 / 26.93 ms**, across **32 MLS bursts**, correlation 1.0 and valid QPC
  packets. A capture with a discontinuity is rejected and rerun, never included.
- Breakdown: driver period **480 frames / 10 ms** (min/default/max all 480),
  native capacity **1,056 / 22 ms**, submission watermark **480 / 10 ms**,
  mixer quantum **128 / 2.667 ms**, cached frames at command **0–96 / 0–2 ms**.
  Command-to-first-submission is recorded per trial (about **1.5–4.7 ms**).
  Queue + period estimates only **20 ms**; the actual **26.93 ms p95** includes
  refill phase/cache and remaining OS/endpoint processing. These overlapping
  measurements must not be added as independent p95 terms. Driver-reported stream
  latency is zero here. QPC loopback excludes DAC, wireless and acoustic propagation.
- The spatial door demo is a two-second left/right render with measured HRTFs:
  **-19.03 LUFS**, **0.898 sample/true peak**, no clipping. Its spectrum and WAV
  hash are preserved in the evidence. Focused native, codec/buffer-boundary,
  block/reference, schema, streaming and effect tests cover the changed paths.

## Native and fixture provenance

Steam Audio's API/runtime is Apache-2.0; the binary SDK includes third-party
notices (including its HRTF/FFT dependencies) that must ship with an export.
The downloader preserves `steamaudio/THIRDPARTY.md` and API headers. See
[Steam Audio source/license](https://github.com/ValveSoftware/steam-audio/tree/v4.8.1).
SDL3 is zlib; its runtime archive's LICENSE.txt is preserved. Rust dependencies
remain Cargo dependencies except the explicitly vendored BSD libopus sources in
`ara-opus-sys`. No Flax/Unity/Unreal source was transcribed.

`tests/fixtures/tone.flac` and `tone.ogg` are Ara-generated 2,400-frame procedural
stereo sawtooths (no external recording), encoded with ffmpeg FLAC/libvorbis and
metadata removed. They verify real incremental FLAC/Vorbis decoding. Goldens
use a fixed integer LCG, fixed scheduling and reviewed WAV BLAKE3 constants.
