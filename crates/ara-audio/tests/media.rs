#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
use ara_audio::{
    codec::{AudioDecoder, MediaDecoder, OpusDecoder, decode_clip},
    importer::{AudioImporter, AudioUse, CookCodec, CookSettings, ImportInput, Importer, pcm_wav},
    streaming::{PackSlice, StreamBudget, channel},
    *,
};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn tone(rate: u32, frames: usize) -> Vec<Frame> {
    (0..frames)
        .map(|i| {
            let s = (i as f32 * std::f32::consts::TAU * 440.0 / rate as f32).sin() * 0.25;
            Frame::new(s, s * 0.7)
        })
        .collect()
}
#[test]
fn pcm_adpcm_and_opus_follow_policy_and_decode() {
    let frames = tone(48000, 4800);
    let wav = pcm_wav(48000, &frames).unwrap();
    for codec in [CookCodec::Pcm, CookCodec::Adpcm, CookCodec::Opus] {
        let usage = if codec == CookCodec::Opus {
            AudioUse::Music
        } else {
            AudioUse::ShortEffect
        };
        let settings = CookSettings {
            usage,
            short_codec: codec,
            ..Default::default()
        };
        let cooked = AudioImporter
            .import(&ImportInput {
                extension: "wav",
                bytes: &wav,
                settings,
            })
            .unwrap();
        assert_eq!(cooked.codec, codec);
        assert_eq!(
            cooked.hash,
            blake3::hash(&cooked.bytes).to_hex().to_string()
        );
        let mut decoder: Box<dyn AudioDecoder> = if codec == CookCodec::Opus {
            Box::new(OpusDecoder::open(Cursor::new(cooked.bytes)).unwrap())
        } else {
            Box::new(MediaDecoder::open(Box::new(Cursor::new(cooked.bytes)), "wav").unwrap())
        };
        let clip = decode_clip(decoder.as_mut(), 1024 * 1024).unwrap();
        assert!(clip.len() >= frames.len(), "{codec:?}: {}", clip.len());
        assert_eq!(clip.len(), frames.len(), "{codec:?}");
        let error = frames
            .iter()
            .zip(clip.frames())
            .map(|(a, b)| (a.left - b.left).powi(2))
            .sum::<f32>()
            / frames.len() as f32;
        assert!(error.sqrt() < 0.025, "{codec:?} RMS error {}", error.sqrt());
    }
    let settings = CookSettings {
        short_codec: CookCodec::Opus,
        ..Default::default()
    };
    assert!(
        AudioImporter
            .import(&ImportInput {
                extension: "wav",
                bytes: &wav,
                settings
            })
            .is_err()
    );
}

#[test]
fn three_minute_music_cooks_and_decodes_with_a_small_pcm_budget() {
    let rate = 48000;
    let count = rate as usize * 181;
    let frames = tone(rate, count);
    let wav = pcm_wav(rate, &frames).unwrap();
    drop(frames);
    let cooked = AudioImporter
        .import(&ImportInput {
            extension: "wav",
            bytes: &wav,
            settings: CookSettings {
                usage: AudioUse::Music,
                decoded_budget: 65536,
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(cooked.codec, CookCodec::Opus);
    assert!(cooked.loudness.is_some());
    assert!(cooked.peak > 0.24);
    let mut decoder = OpusDecoder::open(Cursor::new(cooked.bytes)).unwrap();
    let mut block = [Frame::ZERO; 4096];
    let mut decoded = 0;
    loop {
        let n = decoder.read_frames(&mut block).unwrap();
        if n == 0 {
            break;
        }
        decoded += n;
    }
    assert_eq!(
        decoded, count,
        "Opus EOS trimming must retain the exact duration"
    );
    assert!(
        AudioImporter
            .import(&ImportInput {
                extension: "wav",
                bytes: &wav,
                settings: CookSettings {
                    decoded_budget: 65536,
                    ..Default::default()
                },
            })
            .is_err(),
        "short effects remain subject to the resident budget"
    );
}

#[test]
fn opus_cooking_resamples_and_trims_lookahead() {
    let wav = pcm_wav(44100, &tone(44100, 4410)).unwrap();
    let cooked = AudioImporter
        .import(&ImportInput {
            extension: "wav",
            bytes: &wav,
            settings: CookSettings {
                usage: AudioUse::Dialogue,
                ..Default::default()
            },
        })
        .unwrap();
    let mut decoder = OpusDecoder::open(Cursor::new(cooked.bytes)).unwrap();
    let clip = decode_clip(&mut decoder, 1024 * 1024).unwrap();
    assert_eq!(clip.sample_rate(), 48000);
    assert_eq!(clip.len(), 4800);
}

#[test]
fn decode_from_pack_is_confined_and_budgeted() {
    let wav = pcm_wav(48000, &tone(48000, 200)).unwrap();
    let mut bytes = vec![55; 32];
    bytes.extend(&wav);
    bytes.extend([66; 32]);
    let mut slice = PackSlice::new(Cursor::new(bytes.clone()), 32, wav.len() as u64).unwrap();
    assert!(slice.seek(SeekFrom::Current(-1)).is_err());
    assert!(slice.seek(SeekFrom::End(1)).is_err());
    let mut visible = Vec::new();
    slice.read_to_end(&mut visible).unwrap();
    assert_eq!(visible, wav);
    let source = PackSlice::new(Cursor::new(bytes), 32, wav.len() as u64).unwrap();
    let mut decoder = MediaDecoder::open(Box::new(source), "wav").unwrap();
    assert!(matches!(
        decode_clip(&mut decoder, 100),
        Err(AudioError::Capacity(_))
    ));
    assert!(PackSlice::new(Cursor::new(vec![0; 8]), u64::MAX, 2).is_err());
}

#[test]
fn synchronous_stream_matches_resident_and_releases_aggregate_budget() {
    let frames = tone(48000, 2048);
    let wav = pcm_wav(48000, &frames).unwrap();
    let mut decoder = MediaDecoder::open(Box::new(Cursor::new(wav.clone())), "wav").unwrap();
    let resident = decode_clip(&mut decoder, frames.len() * 8).unwrap();
    let budget = StreamBudget::new(20000);
    let (mut writer, reader) = channel(48000, 2052, &budget).unwrap();
    assert!(channel(48000, 2052, &budget).is_err());
    assert!(budget.used_bytes() > 0);
    let mut decoder = MediaDecoder::open(Box::new(Cursor::new(wav)), "wav").unwrap();
    while writer.fill(&mut decoder).unwrap() > 0 {}
    assert_eq!(reader.buffered_frames(), 2048);
    assert_eq!(reader.underruns(), 0);
    let mut a = AudioEngine::new(EngineConfig::default()).unwrap();
    let mut b = AudioEngine::new(EngineConfig::default()).unwrap();
    a.play(
        BusId::MASTER,
        resident,
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    b.play_stream(
        BusId::MASTER,
        reader,
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        a.render_frames(2304).unwrap(),
        b.render_frames(2304).unwrap()
    );
    drop(b);
    drop(writer);
    assert_eq!(budget.used_bytes(), 0);
}

#[test]
fn corrupt_sources_return_errors_instead_of_silently_playing() {
    assert!(MediaDecoder::open(Box::new(Cursor::new(vec![0; 64])), "wav").is_err());
    assert!(OpusDecoder::open(Cursor::new(vec![0; 64])).is_err());
    assert!(Clip::new(48000, vec![Frame::new(f32::NAN, 0.0)], 8).is_err());
}

#[test]
fn flac_and_vorbis_assets_decode_incrementally() {
    for (extension, bytes) in [
        ("flac", include_bytes!("fixtures/tone.flac").as_slice()),
        ("ogg", include_bytes!("fixtures/tone.ogg").as_slice()),
    ] {
        let mut decoder =
            MediaDecoder::open(Box::new(Cursor::new(bytes.to_vec())), extension).unwrap();
        assert_eq!(decoder.sample_rate(), 48000);
        let decoded = decode_clip(&mut decoder, 64000).unwrap();
        assert_eq!(decoded.len(), 2400, "{extension}");
        assert!(decoded.frames().iter().any(|f| f.left.abs() > 0.1));
    }
}

#[test]
fn abandoned_stream_and_worker_teardown_release_budget() {
    let budget = StreamBudget::new(20000);
    let (writer, reader) = channel(48000, 100, &budget).unwrap();
    drop(writer);
    assert!(reader.ended());
    drop(reader);
    let wav = pcm_wav(48000, &tone(48000, 2048)).unwrap();
    let decoder = MediaDecoder::open(Box::new(Cursor::new(wav)), "wav").unwrap();
    let (reader, worker) = ara_audio::streaming::spawn(Box::new(decoder), 256, &budget).unwrap();
    assert_eq!(reader.buffered_frames(), 256);
    drop(reader);
    worker.finish().unwrap();
    assert_eq!(budget.used_bytes(), 0);
}

#[test]
fn declared_wav_lengths_reject_truncation() {
    let mut wav = pcm_wav(48000, &tone(48000, 2048)).unwrap();
    wav.truncate(wav.len() - 100);
    assert!(MediaDecoder::open(Box::new(Cursor::new(wav)), "wav").is_err());
}
