#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Audio v2 engine mechanisms: aux sends, procedural voices, category limits,
//! loudness normalization, air absorption, capture meters and audibility diagnostics.
use ara_audio::{
    capture::capture_audio, diagnose::Audibility, diagnose::codes, effects::TrackBuilder,
    spatial::AcousticPath, *,
};

fn engine(real: usize) -> AudioEngine {
    AudioEngine::new(EngineConfig {
        real_voices: real,
        total_voices: 16,
        buses: 8,
        limiter: None,
        ..Default::default()
    })
    .unwrap()
}

fn noise(frames: usize, amplitude: f32) -> Clip {
    let mut state = 11_u32;
    let frames = (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let s = ((state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0) * amplitude;
            Frame::new(s, s)
        })
        .collect();
    Clip::new(48000, frames, 1 << 24).unwrap()
}

/// Copies the mono send to both channels: makes routing exactly observable.
struct Mirror;
impl AuxEffect for Mirror {
    fn process(&mut self, input: &[f32], out: &mut [Frame], _: u32) {
        for (frame, s) in out.iter_mut().zip(input) {
            *frame = Frame::new(*s, *s);
        }
    }
}

fn spatial(send: f32) -> Playback {
    Playback {
        looping: true,
        parameters: VoiceParameters {
            direction: [0.0, 0.0, -1.0],
            path: AcousticPath {
                reverb_send: send,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn aux_send_is_per_voice_same_quantum_and_follows_bus_gain() {
    let mut e = engine(4);
    let sfx = e
        .add_bus("sfx", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    let room = e.add_aux("room", Box::new(Mirror)).unwrap();
    e.route_aux(sfx, Some(room)).unwrap();
    let dry = e.play(sfx, noise(4800, 0.3), spatial(0.0)).unwrap();
    let wet = e.play(sfx, noise(4800, 0.3), spatial(0.5)).unwrap();
    let mut out = vec![0.0; 2 * 1024];
    e.render(&mut out).unwrap();
    // Only the voice with a send reaches the aux; the first quantum already has output.
    let first = capture_audio(&mut e, 0.1).unwrap();
    let aux = first
        .analysis
        .buses
        .iter()
        .find(|b| b.name == "room")
        .unwrap();
    assert_eq!(aux.kind, "aux");
    assert!(aux.rms_dbfs.unwrap() > -30.0, "{aux:?}");
    assert!(e.voice_state(dry).is_some() && e.voice_state(wet).is_some());
    // Muting the bus silences the dry signal and the reverb send.
    e.set_bus_gain(sfx, 0.0, 0).unwrap();
    let mut settle = vec![0.0; 2 * 512];
    e.render(&mut settle).unwrap();
    let muted = capture_audio(&mut e, 0.05).unwrap();
    assert!(muted.analysis.silent, "{:?}", muted.analysis.buses);
    assert_eq!(e.muted_ancestor(sfx), Some(sfx));
    assert_eq!(e.effective_gain(sfx), Some(0.0));
}

#[test]
fn aux_output_is_partition_independent() {
    let make = || {
        let mut e = engine(4);
        let room = e.add_aux("room", Box::new(Mirror)).unwrap();
        e.route_aux(BusId::MASTER, Some(room)).unwrap();
        e.play(BusId::MASTER, noise(3000, 0.2), spatial(0.7))
            .unwrap();
        e
    };
    let (mut a, mut b) = (make(), make());
    let mut whole = vec![0.0; 4000];
    a.render(&mut whole).unwrap();
    let mut split = Vec::new();
    for frames in [3, 125, 129, 700, 1043] {
        let mut block = vec![0.0; frames * 2];
        b.render(&mut block).unwrap();
        split.extend(block);
    }
    assert_eq!(whole, split);
}

/// A deterministic 1 kHz sine that ends after `frames`.
struct Sine {
    phase: f32,
    left: usize,
}
impl SignalSource for Sine {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn render(&mut self, out: &mut [Frame], _: u64) {
        for frame in out {
            let s = if self.left > 0 {
                self.left -= 1;
                (self.phase * std::f32::consts::TAU).sin() * 0.5
            } else {
                0.0
            };
            self.phase = (self.phase + 1000.0 / 48000.0).fract();
            *frame = Frame::new(s, s);
        }
    }
    fn ended(&self) -> bool {
        self.left == 0
    }
}

#[test]
fn procedural_voices_render_and_finish() {
    let mut e = engine(4);
    let prepared = PreparedVoice::procedural(
        Box::new(Sine {
            phase: 0.0,
            left: 4800,
        }),
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let id = e.install(BusId::MASTER, prepared).unwrap();
    let c = capture_audio(&mut e, 0.1).unwrap();
    assert!((c.analysis.channel_rms[0] - 0.5 / 2f64.sqrt()).abs() < 0.01);
    let tail = capture_audio(&mut e, 0.05).unwrap();
    assert!(tail.analysis.silent);
    e.collect_finished();
    assert!(e.voice_state(id).is_none());
    assert!(
        PreparedVoice::procedural(
            Box::new(Sine {
                phase: 0.0,
                left: 1
            }),
            Playback {
                looping: true,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn category_limits_virtualize_by_priority_and_free_global_slots() {
    let mut e = engine(3);
    let steps = e
        .add_bus("footsteps", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    let ui = e.add_bus("ui", BusId::MASTER, TrackBuilder::new()).unwrap();
    e.set_voice_limit(steps, Some(1)).unwrap();
    let mut play = |bus, priority| {
        e.play(
            bus,
            noise(2400, 0.1),
            Playback {
                looping: true,
                priority,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let low = play(steps, 150);
    let high = play(steps, 200);
    let a = play(ui, 50);
    let b = play(ui, 40);
    let mut out = vec![0.0; 256];
    e.render(&mut out).unwrap();
    let status = |e: &AudioEngine, id| e.voice_state(id).unwrap().status;
    assert_eq!(status(&e, high), VoiceStatus::Playing);
    assert_eq!(status(&e, low), VoiceStatus::VirtualBusLimit);
    // The skipped footstep's global slot goes to the next candidate.
    assert_eq!(status(&e, a), VoiceStatus::Playing);
    assert_eq!(status(&e, b), VoiceStatus::Playing);
    e.set_voice_limit(steps, None).unwrap();
    e.render(&mut out).unwrap();
    assert_eq!(status(&e, b), VoiceStatus::VirtualGlobalLimit);
}

#[test]
fn loudness_target_normalizes_music_to_minus_16_lufs() {
    let mut e = engine(4);
    let music = e
        .add_bus("music", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    e.set_loudness_target(music, Some(loudness::DEFAULT_TARGET_LUFS))
        .unwrap();
    let quiet = noise(48000 * 2, 0.02).measure_loudness().unwrap();
    let lufs = quiet.loudness().unwrap();
    assert!(lufs < -30.0, "{lufs}");
    let id = e
        .play(
            music,
            quiet,
            Playback {
                looping: true,
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap();
    // The +12 dB cap applies: -16 is not reachable from a ~-36 LUFS source.
    assert!((e.voice_state(id).unwrap().trim - loudness::MAX_BOOST).abs() < 1e-6);
    let loud = noise(48000 * 2, 0.4).measure_loudness().unwrap();
    let mut e = engine(4);
    let music = e
        .add_bus("music", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    e.set_loudness_target(music, Some(-16.0)).unwrap();
    e.play(
        music,
        loud,
        Playback {
            looping: true,
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let c = capture_audio(&mut e, 2.0).unwrap();
    let measured = c.analysis.integrated_lufs.unwrap();
    assert!((measured + 16.0).abs() < 0.5, "{measured}");
}

#[test]
fn air_absorption_removes_highs_and_clear_paths_stay_bit_exact() {
    let render = |air: [f32; 3]| {
        let mut e = engine(4);
        e.play(
            BusId::MASTER,
            noise(48000, 0.3),
            Playback {
                looping: true,
                parameters: VoiceParameters {
                    path: AcousticPath {
                        air,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        capture_audio(&mut e, 0.5).unwrap().analysis
    };
    let clear = render([1.0; 3]);
    let far = render([1.0, 0.7, 0.1]);
    let top = |a: &capture::Analysis| a.spectrum.last().unwrap().rms_dbfs.unwrap();
    let bottom = |a: &capture::Analysis| a.spectrum[1].rms_dbfs.unwrap();
    assert!(
        top(&clear) - top(&far) > 12.0,
        "{} {}",
        top(&clear),
        top(&far)
    );
    assert!((bottom(&clear) - bottom(&far)).abs() < 1.5);
    assert_eq!(clear.wav_blake3, render([1.0; 3]).wav_blake3);
}

#[test]
fn audibility_explains_range_mute_occlusion_and_limits() {
    let mut e = engine(1);
    let sfx = e
        .add_bus("sfx", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    e.set_voice_limit(sfx, Some(1)).unwrap();
    let a = e.play(sfx, noise(2400, 0.1), spatial(0.0)).unwrap();
    let b = e.play(sfx, noise(2400, 0.1), spatial(0.0)).unwrap();
    let mut out = vec![0.0; 256];
    e.render(&mut out).unwrap();
    e.set_bus_gain(sfx, 0.0, 0).unwrap();
    let virtual_voice = [a, b]
        .into_iter()
        .find(|id| e.voice_state(*id).unwrap().status != VoiceStatus::Playing)
        .unwrap();
    let facts = Audibility {
        entity: "e_horn01".into(),
        source: AudioSource {
            max_distance: 20.0,
            ..Default::default()
        },
        loaded: true,
        voice: e.voice_state(virtual_voice),
        muted_bus: e
            .muted_ancestor(sfx)
            .and_then(|b| e.bus_name(b))
            .map(str::to_owned),
        distance: 30.0,
        attenuation: 0.0,
        path: AcousticPath {
            visibility: 0.1,
            ..Default::default()
        },
        occluders: vec!["concrete wall e_wall01".into()],
    };
    let found: Vec<_> = facts.explain().iter().map(|d| d.code).collect();
    for code in [
        codes::OUT_OF_RANGE,
        codes::BUS_MUTED,
        codes::OCCLUDED,
        codes::VOICE_LIMITED,
    ] {
        assert!(found.contains(&code), "{code} missing from {found:?}");
    }
    let range = facts
        .explain()
        .into_iter()
        .find(|d| d.code == codes::OUT_OF_RANGE)
        .unwrap();
    assert_eq!(range.fixes.len(), 1);
}
