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
#[test]
fn a_streamed_payload_reports_progress_and_stages_are_cleaned_up() {
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("setup.exe");
    std::fs::write(&exe, installer("bin/ara.exe")).unwrap();
    let payload = ara_export_installer::Payload::open(&exe).unwrap();
    let size = payload.len();
    let mut hashed = 0;
    let mut verified = payload.verify(|done| hashed = done).unwrap();
    assert_eq!(hashed, size);
    assert_eq!(verified.names(), ["bin/ara.exe"]);
    assert_eq!(verified.total(), b"player bytes".len() as u64);

    let destination = root.path().join("Ara");
    let abandoned = ara_export_installer::Stage::beside(&destination).unwrap();
    let abandoned_path = abandoned.path().to_owned();
    drop(abandoned);
    assert!(!abandoned_path.exists(), "a dropped stage removes itself");

    let stage = ara_export_installer::Stage::beside(&destination).unwrap();
    let mut written = (String::new(), 0);
    verified
        .extract(&stage, |file, done| written = (file.to_owned(), done))
        .unwrap();
    assert_eq!(written, ("bin/ara.exe".to_owned(), verified.total()));
    stage.publish(&destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("bin/ara.exe")).unwrap(),
        b"player bytes"
    );
    let entries = std::fs::read_dir(root.path()).unwrap().count();
    assert_eq!(
        entries, 2,
        "only setup.exe and the published install remain"
    );
}
