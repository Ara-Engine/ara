#![allow(clippy::unwrap_used)]
use ara_audio::{capture::capture_audio, spatial::AcousticPath, steam::SteamAudio, *};

#[test]
#[ignore = "requires pinned Steam Audio runtime; run tools/fetch_runtime.ps1 first"]
fn measured_hrtf_has_direction_occlusion_and_a_repeatable_render() {
    let path = std::env::var_os("ARA_STEAM_AUDIO_LIBRARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("runtime/steam/steamaudio/lib/windows-x64/phonon.dll")
        });
    let steam = SteamAudio::load(path, 48000, None).unwrap();
    let render = |x: f32, visibility: f32| {
        let mut state = 11_u32;
        let clip = Clip::new(
            48000,
            (0..2048)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    let s = ((state >> 16) as i32 - 32768) as f32 / 32768.0 * 0.1;
                    Frame::new(s, s)
                })
                .collect(),
            16384,
        )
        .unwrap();
        let mut e = AudioEngine::new(EngineConfig::default()).unwrap();
        e.play(
            BusId::MASTER,
            clip,
            Playback {
                parameters: VoiceParameters {
                    direction: [x, 0.0, -0.6],
                    path: AcousticPath {
                        visibility,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                spatializer: Some(Box::new(steam.spatializer().unwrap())),
                ..Default::default()
            },
        )
        .unwrap();
        capture_audio(&mut e, 0.2).unwrap()
    };
    let left = render(-0.8, 1.0);
    let right = render(0.8, 1.0);
    let blocked = render(-0.8, 0.0);
    assert!(left.analysis.channel_rms[0] > left.analysis.channel_rms[1]);
    assert!(right.analysis.channel_rms[1] > right.analysis.channel_rms[0]);
    assert!(blocked.analysis.silent);
    assert_eq!(
        left.analysis.wav_blake3,
        render(-0.8, 1.0).analysis.wav_blake3
    );
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    assert_eq!(
        left.analysis.wav_blake3,
        "dacaae00372d3e35c2e748f16473dee24e393f831d0ed483734bee167b9a868b"
    );
}
