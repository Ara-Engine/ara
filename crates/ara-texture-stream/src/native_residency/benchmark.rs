//! Explicit CPU preparation comparison; ordinary tests never enforce timing limits.
use super::*;
use ara_asset_format::{TextureFormat, write_ktx2};
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "explicit compressed versus decoded preparation measurement"]
fn compressed_preparation_measurement() {
    let size = 2048u32;
    for format in [TextureFormat::Bc7Srgb, TextureFormat::Bc5] {
        let levels: Vec<_> = (0..=size.ilog2())
            .map(|level| {
                let extent = (size >> level).max(1);
                let mut blocks =
                    vec![0; (extent.div_ceil(4).pow(2) as usize) * format.block_bytes()];
                for block in blocks.chunks_exact_mut(format.block_bytes()) {
                    // BC7 mode 6 or two constant BC4 channels; both are valid encodings.
                    if format == TextureFormat::Bc7Srgb {
                        block[0] = 0x40;
                    } else {
                        block[0..2].fill(128);
                        block[8..10].fill(128);
                    }
                }
                blocks
            })
            .collect();
        let payload: Arc<[u8]> = write_ktx2(size, size, format, &levels).unwrap().into();
        for repeat in 0..3 {
            let started = Instant::now();
            let decoded = crate::Source::from_cooked(&payload, 1).unwrap();
            let decoded_bytes: usize = decoded.levels.iter().map(Vec::len).sum();
            black_box(&decoded);
            let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
            drop(decoded);
            let started = Instant::now();
            let native = NativeSource::from_cooked(Arc::clone(&payload)).unwrap();
            let staged = native.stage(0).unwrap();
            black_box(&staged);
            let native_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{format:?} repeat={repeat} decoded={decode_ms:.4}ms native={native_ms:.4}ms RGBA={decoded_bytes}B BC={}B",
                staged.len()
            );
        }
    }
}
