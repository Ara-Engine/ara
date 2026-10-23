#![allow(clippy::unwrap_used)]
use ara_opus_sys::{Decoder, Encoder};

#[test]
fn invalid_arguments_are_rejected_before_touching_buffers() {
    assert!(Encoder::new(123, 2).is_err());
    assert!(Decoder::new(48000, 3).is_err());
    let mut encoder = Encoder::new(48000, 2).unwrap();
    assert!(encoder.encode_float(&[0.0; 959], &mut [0; 100]).is_err());
    assert!(encoder.encode_float(&[0.0; 200], &mut [0; 100]).is_err());
    let mut decoder = Decoder::new(48000, 2).unwrap();
    assert!(decoder.decode_float(&[], &mut [0.0; 1920]).is_err());
    assert!(decoder.decode_float(&[0], &mut [0.0; 1919]).is_err());
}

#[test]
fn owned_codec_moves_to_worker_and_respects_pcm_bounds() {
    let encoder = Encoder::new(48000, 2).unwrap();
    std::thread::spawn(move || {
        let mut encoder = encoder;
        encoder.set_bitrate(128000).unwrap();
        assert!(encoder.get_lookahead().unwrap() > 0);
        let pcm = std::array::from_fn::<_, 1920, _>(|i| (i as f32 * 0.02).sin() * 0.25);
        let mut packet = [0; 4096];
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        let mut output = [123.0; 1924];
        let frames = Decoder::new(48000, 2)
            .unwrap()
            .decode_float(&packet[..len], &mut output[2..1922])
            .unwrap();
        assert_eq!(frames, 960);
        assert_eq!(output[..2], [123.0; 2]);
        assert_eq!(output[1922..], [123.0; 2]);
        assert!(
            output[2..1922]
                .iter()
                .all(|s| s.is_finite() && s.abs() <= 1.0)
        );
    })
    .join()
    .unwrap();
}
