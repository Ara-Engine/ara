#![allow(clippy::unwrap_used)]
use std::io::Write;

fn installer(name: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"player bytes").unwrap();
    let archive = zip.finish().unwrap().into_inner();
    ara_export_installer::append(b"MZ stub", &archive).unwrap()
}
#[test]
fn install_validates_payload_then_publishes_without_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("game");
    let bytes = installer("game/Game.dll");
    ara_export_installer::install(&bytes, &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("game/Game.dll")).unwrap(),
        b"player bytes"
    );
    assert!(ara_export_installer::install(&bytes, &destination).is_err());
    let mut corrupt = bytes;
    corrupt[10] ^= 1;
    assert!(ara_export_installer::install(&corrupt, &root.path().join("bad")).is_err());
    assert!(!root.path().join("bad").exists());
}
#[test]
fn traversal_devices_and_alternate_streams_are_rejected_before_writing() {
    for name in [
        "../escaped",
        "/rooted",
        "CON.txt",
        "LPT1",
        "foo/bar.",
        "foo/a:b",
        "foo\\bar",
        "NUL",
        "a/../b",
    ] {
        assert!(
            ara_export_installer::list(&installer(name)).is_err(),
            "{name}"
        );
    }
}
