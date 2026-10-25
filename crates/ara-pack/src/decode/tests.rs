#![allow(clippy::unwrap_used)]
use crate::{CookKey, Input, OwnedPack};

#[test]
fn bounded_decoder_preserves_raw_and_file_integrity_checks() {
    let bytes: Vec<_> = (0..20_000).map(|i| (i * 37) as u8).collect();
    let built = crate::build(
        [Input {
            path: "data".into(),
            bytes: bytes.clone(),
        }],
        CookKey {
            platform: "windows".into(),
            arch: "x64".into(),
            config: "release".into(),
            cooker: "test".into(),
            settings: Default::default(),
        },
        8192,
        None,
    )
    .unwrap();
    let owner = OwnedPack::open(built.bytes).unwrap();
    let mut pack = owner.view();
    assert_eq!(pack.read("data", 20_000, None).unwrap(), bytes);
    assert!(pack.read("data", 19_999, None).is_err());
    let id = pack.manifest().entries["data"].chunks[0].clone();
    let size = pack.manifest().chunks[&id].raw;
    for wrong in [size - 1, size + 1] {
        pack.manifest.to_mut().chunks.get_mut(&id).unwrap().raw = wrong;
        assert!(pack.read_chunk(&id, None).is_err());
    }
    pack.manifest.to_mut().chunks.get_mut(&id).unwrap().raw = size;
    let compressed = pack.stored_chunk(&id).unwrap();
    assert_eq!(
        super::chunk(compressed, 32).unwrap().len(),
        33,
        "the decoder stops after the bounded overflow byte"
    );
    pack.manifest.to_mut().entries.get_mut("data").unwrap().hash = "0".repeat(64);
    assert!(pack.read("data", 20_000, None).is_err());
    let chunk = pack.manifest.to_mut().chunks.remove(&id).unwrap();
    pack.manifest.to_mut().chunks.insert("1".repeat(64), chunk);
    assert!(pack.read_chunk(&"1".repeat(64), None).is_err());
}

#[cfg(feature = "native-decode")]
#[test]
fn native_decoder_rejects_an_oversized_window() {
    use std::io::Read;
    // Empty last raw block, no content size, a 32 MiB history window. Valid
    // zstd framing, but it exceeds the pack's 16 MiB chunk/window contract.
    let frame = [0x28, 0xb5, 0x2f, 0xfd, 0, 0x78, 1, 0, 0];
    let mut unrestricted = zstd::stream::read::Decoder::with_buffer(frame.as_slice()).unwrap();
    let mut output = Vec::new();
    unrestricted.read_to_end(&mut output).unwrap();
    assert!(output.is_empty(), "fixture is a valid empty zstd frame");
    assert!(super::chunk(&frame, 1).is_err());
}
