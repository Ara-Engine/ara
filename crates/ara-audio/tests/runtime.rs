#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
use ara_audio::{
    capture::capture_audio, effects::TrackBuilder, schema::Attenuation, spatial::evaluate, *,
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
fn clip() -> Clip {
    let mut state = 7_u32;
    let frames = (0..1024)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let sample = ((state >> 16) as i32 - 32768) as f32 / 131072.0;
            Frame::new(sample, sample * 0.5)
        })
        .collect();
    Clip::new(48000, frames, 8192).unwrap()
}

#[test]
fn reflected_defaults_and_docs_match_runtime() {
    let mut db = ara_schema::SchemaDb::new();
    register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for (name, value) in [
        (
            "AudioSource",
            serde_json::to_value(AudioSource::default()).unwrap(),
        ),
        (
            "AudioListener",
            serde_json::to_value(AudioListener::default()).unwrap(),
        ),
        (
            "AcousticPath",
            serde_json::to_value(ara_audio::spatial::AcousticPath::default()).unwrap(),
        ),
        (
            "VoiceParameters",
            serde_json::to_value(VoiceParameters::default()).unwrap(),
        ),
    ] {
        let desc = db.get(name).unwrap();
        for field in desc.fields() {
            assert!(!field.doc.is_empty());
            assert!(
                ara_json::json_eq(
                    &db.canonical_value(&field.ty, &value[&field.name]),
                    &db.canonical_value(&field.ty, &db.field_default(field))
                ),
                "{name}.{}",
                field.name
            );
        }
    }
}

#[test]
fn output_golden_and_partition_independence() {
    let make = || {
        let mut e = engine(4);
        e.play(
            BusId::MASTER,
            clip(),
            Playback {
                looping: true,
                parameters: VoiceParameters {
                    direction: [-0.6, 0.2, -0.8],
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        e
    };
    let mut a = make();
    let mut b = make();
    let mut whole = vec![0.0; 4096];
    a.render(&mut whole).unwrap();
    let mut split = Vec::new();
    for frames in [1, 17, 128, 33, 700, 1169] {
        let mut block = vec![0.0; frames * 2];
        b.render(&mut block).unwrap();
        split.extend(block);
    }
    assert_eq!(whole, split);
    let capture = ara_audio::capture::analyze(48000, &whole, 0).unwrap();
    assert_eq!(
        capture.analysis.wav_blake3,
        "c84dd515c83527212d09d09b335458e9702e69984a20a00a6bc82fd8c7c34c03"
    );
}

#[test]
fn sample_clock_starts_and_stops_inside_a_quantum() {
    let mut e = engine(1);
    let c = Clip::new(48000, vec![Frame::new(0.5, 0.25); 100], 800).unwrap();
    let id = e
        .play(
            BusId::MASTER,
            c,
            Playback {
                at_frame: 17,
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap();
    e.command(
        id,
        VoiceCommand::Stop {
            at_frame: 21,
            fade_frames: 0,
        },
    )
    .unwrap();
    let samples = e.render_frames(128).unwrap();
    assert!(samples[..17].iter().all(|f| *f == Frame::ZERO));
    assert!(samples[17..21].iter().all(|f| *f == Frame::new(0.5, 0.25)));
    assert!(samples[21..].iter().all(|f| *f == Frame::ZERO));
}

#[test]
fn future_impulse_keeps_its_first_sample_after_waiting_multiple_quanta() {
    let mut e = engine(1);
    let prepared = std::thread::spawn(|| {
        PreparedVoice::resident(
            Clip::new(48000, vec![Frame::new(0.5, 0.5)], 8).unwrap(),
            Playback {
                at_frame: 257,
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap()
    })
    .join()
    .unwrap();
    e.render_frames(128).unwrap();
    e.install(BusId::MASTER, prepared).unwrap();
    let samples = e.render_frames(256).unwrap();
    assert!(samples[..129].iter().all(|s| *s == Frame::ZERO));
    assert_eq!(samples[129].left, 0.5);
}

#[test]
fn stopping_a_future_voice_releases_it_before_its_start_time() {
    let mut e = engine(1);
    let id = e
        .play(
            BusId::MASTER,
            clip(),
            Playback {
                at_frame: 480000,
                ..Default::default()
            },
        )
        .unwrap();
    e.command(
        id,
        VoiceCommand::Stop {
            at_frame: 10,
            fade_frames: 0,
        },
    )
    .unwrap();
    assert!(
        e.render_frames(256)
            .unwrap()
            .iter()
            .all(|f| *f == Frame::ZERO)
    );
    e.collect_finished();
    assert_eq!(e.voice_position(id), None);
}

#[test]
fn partial_captures_count_only_delivered_overloads() {
    let mut e = engine(1);
    e.play(
        BusId::MASTER,
        Clip::new(48000, vec![Frame::new(2.0, 2.0); 128], 1024).unwrap(),
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let first = capture_audio(&mut e, 1.0 / 48000.0).unwrap();
    assert_eq!(first.analysis.pre_limiter_overloads, 2);
    let second = capture_audio(&mut e, 1.0 / 48000.0).unwrap();
    assert_eq!(second.analysis.pre_limiter_overloads, 2);
}

#[test]
fn gain_changes_smooth_at_the_exact_sample() {
    let mut e = engine(1);
    let c = Clip::new(48000, vec![Frame::new(0.5, 0.5); 200], 1600).unwrap();
    let id = e
        .play(
            BusId::MASTER,
            c,
            Playback {
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap();
    e.command(
        id,
        VoiceCommand::Set {
            at_frame: 20,
            parameters: VoiceParameters {
                gain: 0.0,
                ..Default::default()
            },
            ramp_frames: 4,
        },
    )
    .unwrap();
    let f = e.render_frames(128).unwrap();
    assert_eq!(f[19].left, 0.5);
    assert_eq!(f[20].left, 0.5);
    assert_eq!(f[21].left, 0.375);
    assert_eq!(f[22].left, 0.25);
    assert_eq!(f[23].left, 0.125);
    assert_eq!(f[24].left, 0.0);
}

#[test]
fn priorities_virtualize_without_freezing_time_and_resume() {
    let mut e = engine(1);
    let low = e
        .play(
            BusId::MASTER,
            clip(),
            Playback {
                priority: 1,
                looping: true,
                ..Default::default()
            },
        )
        .unwrap();
    let high = e
        .play(
            BusId::MASTER,
            clip(),
            Playback {
                priority: 255,
                looping: true,
                ..Default::default()
            },
        )
        .unwrap();
    e.render_frames(256).unwrap();
    assert_eq!(e.voice_position(low), Some(256));
    assert_eq!(e.voice_position(high), Some(256));
    e.command(
        high,
        VoiceCommand::Stop {
            at_frame: 256,
            fade_frames: 0,
        },
    )
    .unwrap();
    e.render_frames(256).unwrap();
    assert_eq!(e.voice_position(low), Some(512));
    let f = e.render_frames(128).unwrap();
    assert!(f.iter().any(|f| f.left.abs() > 0.01));
}

#[test]
fn nested_bus_snapshot_and_rejection_are_atomic() {
    let mut e = engine(1);
    let parent = e
        .add_bus("sfx", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    let child = e.add_bus("doors", parent, TrackBuilder::new()).unwrap();
    e.apply_snapshot(
        &Snapshot {
            name: "quiet".into(),
            gains: vec![(parent, 0.5), (child, 0.5)],
        },
        0,
    )
    .unwrap();
    assert!(
        e.apply_snapshot(
            &Snapshot {
                name: "bad".into(),
                gains: vec![(parent, 1.0), (child, f32::NAN)]
            },
            0
        )
        .is_err()
    );
    let c = Clip::new(48000, vec![Frame::new(0.5, 0.5); 1000], 8000).unwrap();
    e.play(
        child,
        c,
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let f = e.render_frames(128).unwrap();
    assert!((f[127].left - 0.125).abs() < 0.00001);
}

#[test]
fn sample_commands_reject_past_order_errors_and_overflow() {
    let mut e = engine(1);
    let id = e.play(BusId::MASTER, clip(), Playback::default()).unwrap();
    e.command(
        id,
        VoiceCommand::Stop {
            at_frame: 100,
            fade_frames: 1,
        },
    )
    .unwrap();
    assert!(
        e.command(
            id,
            VoiceCommand::Stop {
                at_frame: 99,
                fade_frames: 1
            }
        )
        .is_err()
    );
    for i in 0..63 {
        e.command(
            id,
            VoiceCommand::Stop {
                at_frame: 101 + i,
                fade_frames: 1,
            },
        )
        .unwrap();
    }
    assert!(matches!(
        e.command(
            id,
            VoiceCommand::Stop {
                at_frame: 200,
                fade_frames: 1
            }
        ),
        Err(AudioError::Capacity(_))
    ));
    e.render_frames(128).unwrap();
    assert!(
        e.command(
            id,
            VoiceCommand::Stop {
                at_frame: 0,
                fade_frames: 0
            }
        )
        .is_err()
    );
}

#[test]
fn spatial_rolloff_doppler_and_both_cones() {
    let mut source = AudioSource {
        position: [10.0, 0.0, 0.0],
        velocity: [-20.0, 0.0, 0.0],
        ..Default::default()
    };
    let listener = AudioListener::default();
    let spatial = evaluate(&source, &listener).unwrap();
    assert_eq!(spatial.direction, [1.0, 0.0, 0.0]);
    assert!(spatial.doppler > 1.0);
    let inverse = spatial.attenuation;
    source.attenuation = Attenuation::InverseSquare;
    assert!(evaluate(&source, &listener).unwrap().attenuation < inverse);
    source.cone_inner = 30.0;
    source.cone_outer = 60.0;
    source.cone_outer_gain = 0.0;
    assert_eq!(evaluate(&source, &listener).unwrap().attenuation, 0.0);
    source.cone_inner = 360.0;
    source.cone_outer = 360.0;
    let narrow = AudioListener {
        cone_inner: 30.0,
        cone_outer: 60.0,
        cone_outer_gain: 0.0,
        ..listener
    };
    assert_eq!(evaluate(&source, &narrow).unwrap().attenuation, 0.0);
    source.position = [100.0, 0.0, 0.0];
    assert_eq!(evaluate(&source, &listener).unwrap().attenuation, 0.0);
    source.position[0] = f32::NAN;
    assert!(evaluate(&source, &listener).is_err());
}

#[test]
fn agent_capture_distinguishes_silence_safe_spatial_audio_and_overload() {
    let mut silent = engine(1);
    let c = capture_audio(&mut silent, 0.5).unwrap();
    assert!(c.analysis.silent);
    assert_eq!(c.analysis.integrated_lufs, None);
    assert!(!c.analysis.clipping);
    assert!(ara_json::parse(&c.analysis_json().unwrap()).is_ok());
    let mut e = AudioEngine::new(EngineConfig {
        limiter: Some(0.98),
        ..Default::default()
    })
    .unwrap();
    let loud = Clip::new(48000, vec![Frame::new(2.0, 2.0); 24000], 192000).unwrap();
    e.play(
        BusId::MASTER,
        loud,
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let c = capture_audio(&mut e, 0.5).unwrap();
    assert!(c.analysis.clipping);
    assert!(c.analysis.pre_limiter_overloads > 0);
    assert!(c.analysis.sample_peak <= 0.98001);
    assert!(c.analysis.integrated_lufs.is_some());
    assert!(capture_audio(&mut e, f64::NAN).is_err());
    assert!(capture_audio(&mut e, 61.0).is_err());
}
