#![allow(clippy::unwrap_used)]
use ara_export::distribution;
use std::{collections::BTreeMap, io::Read};

#[test]
fn portable_archives_are_deterministic_and_preserve_the_player_payload() {
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build");
    std::fs::create_dir_all(build.join("data/game")).unwrap();
    std::fs::write(build.join("Game"), b"\x7fELF fake player").unwrap();
    std::fs::write(build.join("data/game/Game.so"), b"\x7fELF fake game").unwrap();
    std::fs::write(build.join("data/ara-launch.json"), b"{}\n").unwrap();
    std::fs::write(build.join("THIRD_PARTY_NOTICES.txt"), b"fixture notices\n").unwrap();
    for n in [1, 2] {
        distribution::portable_zip(&build, &root.path().join(format!("{n}.zip"))).unwrap();
        distribution::tar_gz(&build, &root.path().join(format!("{n}.tar.gz"))).unwrap();
    }
    for extension in ["zip", "tar.gz"] {
        assert_eq!(
            std::fs::read(root.path().join(format!("1.{extension}"))).unwrap(),
            std::fs::read(root.path().join(format!("2.{extension}"))).unwrap()
        );
    }
    let zip = ara_export::android::zip_entries(&root.path().join("1.zip")).unwrap();
    assert_eq!(zip["Game"], b"\x7fELF fake player");
    let gzip =
        flate2::read::GzDecoder::new(std::fs::File::open(root.path().join("1.tar.gz")).unwrap());
    let mut archive = tar::Archive::new(gzip);
    let mut files = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        assert_eq!(entry.header().mtime().unwrap(), 0);
        if name == "Game" {
            assert_eq!(entry.header().mode().unwrap(), 0o755);
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        files.insert(name, bytes);
    }
    assert_eq!(zip, files);
}

#[test]
fn package_configuration_rejects_platform_and_release_signing_mismatches() {
    let base = serde_json::json!({"$schema":"ara://schema/export/1","platform":"windows","scriptProject":"Game.csproj","assembly":"Game","content":ara_doc::AUTHORING_ROOT,"templates":[{"platform":"windows","arch":"x64","config":"release","hash":"0".repeat(64),"source":"runtime"}],"packaging":"linuxTar"});
    assert!(ara_export::ExportPreset::from_text(&base.to_string()).is_err());
    let mut value = base.clone();
    value["packaging"] = "windowsPortable".into();
    let preset = ara_export::ExportPreset::from_text(&value.to_string()).unwrap();
    let text = preset.to_text().unwrap();
    assert_eq!(
        text,
        ara_export::ExportPreset::from_text(&text)
            .unwrap()
            .to_text()
            .unwrap()
    );
    assert!(preset.crash.upload_endpoint.is_none());
    value["packaging"] = "windowsInstaller".into();
    assert!(ara_export::ExportPreset::from_text(&value.to_string()).is_err());
}

#[test]
fn appimage_embeds_and_verifies_nested_payloads_on_the_build_host() {
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build");
    std::fs::create_dir_all(build.join("data/game")).unwrap();
    std::fs::write(build.join("Outpost"), b"\x7fELF player").unwrap();
    std::fs::write(build.join("data/game/Game.so"), b"\x7fELF script").unwrap();
    let mut header = vec![0u8; 4096];
    header[..4].copy_from_slice(b"\x7fELF");
    header[8..11].copy_from_slice(b"AI\x02");
    header[18..20].copy_from_slice(&62u16.to_le_bytes());
    let runtime = root.path().join("runtime");
    std::fs::write(&runtime, &header).unwrap();
    let image = root.path().join("game.AppImage");
    distribution::appimage(&build, &runtime, &image).unwrap();
    let bytes = std::fs::read(&image).unwrap();
    assert_eq!(&bytes[..4096], header);
    assert_eq!(&bytes[4096..4100], b"hsqs");
    assert!(bytes.len() > 4096);
    let fs = backhand::FilesystemReader::from_reader_with_offset(
        std::io::BufReader::new(std::fs::File::open(image).unwrap()),
        4096,
    )
    .unwrap();
    for name in ["ara-game.svg", ".DirIcon"] {
        let node = fs
            .files()
            .find(|node| node.fullpath == std::path::Path::new("/").join(name))
            .unwrap();
        let backhand::InnerNode::File(file) = &node.inner else {
            panic!("{name} must be a file");
        };
        let mut icon = Vec::new();
        fs.file(file)
            .reader_checked()
            .unwrap()
            .read_to_end(&mut icon)
            .unwrap();
        assert_eq!(
            icon,
            include_bytes!("../../../assets/brand/icons/game/ara-game.svg")
        );
    }
    let node = fs
        .files()
        .find(|node| node.fullpath == std::path::Path::new("/AppRun"))
        .unwrap();
    let backhand::InnerNode::File(file) = &node.inner else {
        panic!("AppRun must be a file")
    };
    let mut launch = String::new();
    fs.file(file)
        .reader_checked()
        .unwrap()
        .read_to_string(&mut launch)
        .unwrap();
    assert!(launch.contains("/'Outpost'"));
    assert!(!launch.contains("ara-player"));
}
