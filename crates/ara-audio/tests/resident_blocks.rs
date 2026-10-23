#![allow(clippy::unwrap_used)]
use ara_audio::*;

// No-op commands inside each quantum force the sample-by-sample resampler.
// Compare loop seams, virtual advancement/promotion, and a later Doppler change.
#[test]
fn resident_blocks_match_sample_path_through_wraps_and_promotion() {
    for len in [1, 2, 3, 7, 129, 1024] {
        for virtualize in [false, true] {
            let render = |reference: bool| {
                let mut engine = AudioEngine::new(EngineConfig {
                    real_voices: 1,
                    total_voices: 4,
                    limiter: None,
                    ..Default::default()
                })
                .unwrap();
                let parameters = VoiceParameters {
                    gain: 0.125,
                    ..Default::default()
                };
                let clip = Clip::new(
                    48000,
                    (0..len)
                        .map(|i| Frame::new((i as f32 + 1.0) / len as f32, -0.25))
                        .collect(),
                    len * 8,
                )
                .unwrap();
                let id = engine
                    .play(
                        BusId::MASTER,
                        clip.clone(),
                        Playback {
                            looping: true,
                            priority: 1,
                            parameters,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                if virtualize {
                    let high = engine
                        .play(
                            BusId::MASTER,
                            clip,
                            Playback {
                                looping: true,
                                priority: 255,
                                spatial: false,
                                parameters: VoiceParameters {
                                    gain: 0.0,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    engine
                        .command(
                            high,
                            VoiceCommand::Stop {
                                at_frame: 777,
                                fade_frames: 0,
                            },
                        )
                        .unwrap();
                }
                if reference {
                    for block in 0..20 {
                        engine
                            .command(
                                id,
                                VoiceCommand::Set {
                                    at_frame: block * 128 + 1,
                                    parameters,
                                    ramp_frames: 0,
                                },
                            )
                            .unwrap();
                    }
                }
                engine
                    .command(
                        id,
                        VoiceCommand::Set {
                            at_frame: 20 * 128,
                            parameters: VoiceParameters {
                                rate: 1.125,
                                ..parameters
                            },
                            ramp_frames: 19,
                        },
                    )
                    .unwrap();
                engine.render_frames(4096).unwrap()
            };
            assert_eq!(
                render(false),
                render(true),
                "len={len}, virtual={virtualize}"
            );
        }
    }
}

// Gain ramps stay on the contiguous path and follow the smoother sample by sample.
#[test]
fn gain_ramps_on_resident_blocks_are_sample_exact() {
    let mut engine = AudioEngine::new(EngineConfig {
        limiter: None,
        ..Default::default()
    })
    .unwrap();
    let clip = Clip::new(48000, vec![Frame::new(0.5, 0.25); 4096], 4096 * 8).unwrap();
    let id = engine
        .play(
            BusId::MASTER,
            clip,
            Playback {
                looping: true,
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap();
    engine
        .command(
            id,
            VoiceCommand::Set {
                at_frame: 128,
                parameters: VoiceParameters {
                    gain: 0.0,
                    ..Default::default()
                },
                ramp_frames: 256,
            },
        )
        .unwrap();
    let mut out = vec![0.0; 1024 * 2];
    engine.render(&mut out).unwrap();
    let step = -1.0_f32 / 256.0;
    let mut gain = 1.0_f32;
    for frame in 0..512 {
        if frame >= 128 && frame < 384 {
            assert_eq!(out[frame * 2], 0.5 * gain, "frame {frame}");
            gain += step;
        }
    }
    assert!(out[400 * 2..].iter().all(|s| *s == 0.0));
}
