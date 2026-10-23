#![allow(clippy::unwrap_used)]
use ara_pack::{CookKey, EncryptionKey, Input, Pack, build, build_directory, diff};
use std::collections::BTreeMap;

fn cook() -> CookKey {
    CookKey {
        platform: "windows".into(),
        arch: "x64".into(),
        config: "release".into(),
        cooker: "test/1".into(),
        settings: BTreeMap::new(),
    }
}
fn input(path: &str, bytes: &[u8]) -> Input {
    Input {
        path: path.into(),
        bytes: bytes.into(),
    }
}

#[test]
fn deterministic_across_rebuilds_order_roots_and_json_formatting() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("scene.json"), r#"{"b":2,"a":1.0}"#).unwrap();
    std::fs::write(b.path().join("scene.json"), "{\n\"a\":1,\"b\":2\n}").unwrap();
    std::fs::write(a.path().join("mesh.bin"), b"geometry").unwrap();
    std::fs::write(b.path().join("mesh.bin"), b"geometry").unwrap();
    let first = build_directory(a.path(), cook(), 1024, None).unwrap();
    assert_eq!(
        first.bytes,
        build_directory(a.path(), cook(), 1024, None).unwrap().bytes
    );
    assert_eq!(
        first.bytes,
        build_directory(b.path(), cook(), 1024, None).unwrap().bytes
    );
    let forward = build([input("a", b"same"), input("b", b"same")], cook(), 4, None).unwrap();
    let reversed = build([input("b", b"same"), input("a", b"same")], cook(), 4, None).unwrap();
    assert_eq!(forward.bytes, reversed.bytes);
    assert_eq!(forward.manifest.chunks.len(), 1);
}

#[test]
fn delivery_diff_downloads_only_changed_chunks_and_reconstructs() {
    let a = build([input("data", b"aaaabbbbcccc")], cook(), 4, None).unwrap();
    let b = build([input("data", b"aaaaxxxxcccc")], cook(), 4, None).unwrap();
    let delta = diff(&a.manifest, &b.manifest).unwrap();
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.removed.len(), 1);
    let old = Pack::open(&a.bytes).unwrap();
    let new = Pack::open(&b.bytes).unwrap();
    for id in new
        .manifest()
        .chunks
        .keys()
        .filter(|id| !delta.added.contains(*id))
    {
        assert_eq!(old.stored_chunk(id).unwrap(), new.stored_chunk(id).unwrap());
    }
    assert_eq!(new.read("data", 12, None).unwrap(), b"aaaaxxxxcccc");
    let mut other = b.manifest.clone();
    other.cook.platform = "android".into();
    assert!(diff(&a.manifest, &other).is_err());
}

#[test]
fn authenticated_encryption_is_deterministic_and_requires_matching_key() {
    let key = EncryptionKey([7; 32]);
    let first = build([input("data", b"secret content")], cook(), 8, Some(&key)).unwrap();
    assert_eq!(
        first.bytes,
        build([input("data", b"secret content")], cook(), 8, Some(&key))
            .unwrap()
            .bytes
    );
    let pack = Pack::open(&first.bytes).unwrap();
    assert!(pack.read("data", 32, None).is_err());
    assert!(
        pack.read("data", 32, Some(&EncryptionKey([8; 32])))
            .is_err()
    );
    assert_eq!(
        pack.read("data", 32, Some(&key)).unwrap(),
        b"secret content"
    );
    pack.validate_all(Some(&key)).unwrap();
}

#[test]
fn validates_mapping_corruption_ranges_and_caller_limits() {
    let built = build([input("data", b"content")], cook(), 4, None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("immutable.arapack");
    std::fs::write(&path, &built.bytes).unwrap();
    // SAFETY: this test owns the private file and never modifies it while mapped.
    let mapped = unsafe { ara_pack::MappedPack::open(&path) }.unwrap();
    assert_eq!(
        mapped.pack().unwrap().read("data", 7, None).unwrap(),
        b"content"
    );
    assert!(mapped.pack().unwrap().read("data", 6, None).is_err());
    let mut corrupt = built.bytes.clone();
    corrupt[64] ^= 1;
    assert!(Pack::open(&corrupt).is_err());
    let offset = u64::from_le_bytes(built.bytes[24..32].try_into().unwrap()) as usize;
    corrupt = built.bytes.clone();
    corrupt[offset] ^= 1;
    assert!(Pack::open(&corrupt).is_err());
    corrupt = built.bytes.clone();
    corrupt[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Pack::open(&corrupt).is_err());
    for len in [0, 1, 63, 64, 65, built.bytes.len() - 4096] {
        assert!(Pack::open(&built.bytes[..len]).is_err());
    }
}

#[test]
fn rejects_ambiguous_paths_duplicates_and_bad_chunk_sizes() {
    for p in [
        "../x",
        "a/../b",
        "/absolute",
        "C:/x",
        "a\\b",
        "@scene",
        "a//b",
    ] {
        assert!(build([input(p, b"a")], cook(), 4, None).is_err());
    }
    assert!(build([input("a", b"1"), input("a", b"2")], cook(), 4, None).is_err());
    assert!(build([], cook(), 0, None).is_err());
    let built = build([input("empty", b"")], cook(), 4, None).unwrap();
    assert!(
        Pack::open(&built.bytes)
            .unwrap()
            .read("empty", 0, None)
            .unwrap()
            .is_empty()
    );
    let mut other = cook();
    other.platform = "android".into();
    assert_ne!(cook().digest().unwrap(), other.digest().unwrap());
}
