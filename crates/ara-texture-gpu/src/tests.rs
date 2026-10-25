//! Exercise the unified host, immutable manifests, transfer reuse and split kernels.
use ara_asset::{BlockEncoder, Cancellation, CookPreset};
use ara_asset_format::TextureFormat;

fn blocks() -> Vec<[u32; 16]> {
    let mut blocks = vec![[0; 16], [u32::MAX; 16], [0xff9a7834; 16]];
    blocks.push(std::array::from_fn(|i| {
        let x = (i % 4) as u32;
        let y = (i / 4) as u32;
        x * 85 | (y * 85 << 8) | ((x + y) * 42 << 16) | 0xff000000
    }));
    let mut random = 29u32;
    for _ in 0..96 {
        blocks.push(std::array::from_fn(|_| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random
        }));
    }
    blocks
}

#[test]
#[ignore = "requires Vulkan; run through ara-dev gpu with a private profile"]
fn unified_host_matches_integer_oracles_and_reuses_transfers() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = root.join("../../target/ara-texture-gpu/host-probe");
    std::fs::create_dir_all(&scratch)?;
    let cache = tempfile::tempdir_in(scratch)?;
    let encoder = crate::Encoder::new(&root.join("cooked"), cache.path())?;
    let cases = blocks();
    let words: Vec<_> = cases.iter().flatten().copied().collect();
    let cancel = Cancellation::default();
    for (preset, id) in [(CookPreset::Fast,0), (CookPreset::Balanced,1), (CookPreset::HighQuality,2)] {
        for format in [TextureFormat::Bc4, TextureFormat::Bc5, TextureFormat::Bc7] {
            let expected: Vec<_> = cases.iter().flat_map(|block| {
                let words = match format {
                    TextureFormat::Bc4 => crate::bc45_reference::encode_bc4(block, id).to_vec(),
                    TextureFormat::Bc5 => crate::bc45_reference::encode_bc5(block, id).to_vec(),
                    _ => crate::bc7_reference::encode_bc7(block, id).to_vec(),
                };
                words.into_iter().flat_map(u32::to_le_bytes)
            }).collect();
            assert_eq!(encoder.encode(format, preset, &words, &cancel)?, expected);
        }
        let hdr: Vec<[u32; 32]> = (0..12).map(|index| std::array::from_fn(|word| {
            let r = ((word * 719 + index * 163) % 0x7c00) as u32;
            let g = ((word * 137 + index * 641) % 0x7c00) as u32;
            if word % 2 == 0 { r | (g << 16) } else { r }
        })).collect();
        let expected: Vec<_> = hdr.iter().flat_map(|block| crate::bc6h_reference::encode_bc6h(block, id).into_iter().flat_map(u32::to_le_bytes)).collect();
        let words: Vec<_> = hdr.iter().flatten().copied().collect();
        assert_eq!(encoder.encode(TextureFormat::Bc6h, preset, &words, &cancel)?, expected);
    }
    assert!(encoder.encode(TextureFormat::Bc7, CookPreset::Balanced, &[], &cancel)?.is_empty());
    assert!(encoder.encode(TextureFormat::Bc7, CookPreset::Balanced, &[0; 15], &cancel).is_err());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(encoder.encode(TextureFormat::Bc7, CookPreset::Balanced, &words, &cancelled), Err(ara_asset::Error::Cancelled)));
    // Cross the guaranteed Vulkan dispatch-X bound; the tail must not include
    // stale bytes left in the reused readback allocation by the first chunk.
    let count = crate::dispatch::MAX_BLOCKS + 7;
    let constant = [0xff9a7834; 16];
    let words: Vec<_> = (0..count).flat_map(|_| constant).collect();
    let expected: Vec<_> = crate::bc7_reference::encode_bc7(&constant, 1).into_iter().flat_map(u32::to_le_bytes).collect();
    let encoded = encoder.encode(TextureFormat::Bc7, CookPreset::Balanced, &words, &cancel)?;
    assert_eq!(encoded.len(), count * 16);
    assert!(encoded.chunks_exact(16).all(|block| block == expected));
    assert!(encoder.metrics().get("Texture.Encode.gpu_ms").is_some_and(|ms| *ms > 0.0));
    Ok(())
}
