#![allow(clippy::unwrap_used)]
use ara_update::{testing, verify};
use std::{fs, process::Command};

#[test]
fn real_publication_rejects_rehearsal_packages_and_github_oversized_assets() {
    let root = testing::scratch("publish-fences");
    let prefix = "ara-hub-0.0.1-alpha-windows-x64";
    fs::create_dir(root.join(prefix)).unwrap();
    fs::write(
        root.join(prefix).join("hub.json"),
        br#"{"format":"ara.hub/1","version":"0.0.1-alpha","rehearsal":true}"#,
    )
    .unwrap();
    fs::write(root.join(prefix).join("THIRD_PARTY_NOTICES.txt"), "notices").unwrap();
    fs::write(root.join("HUB_RELEASE_NOTES.md"), "Fixture").unwrap();
    let archive = root.join(format!("{prefix}.zip"));
    fs::write(&archive, b"archive").unwrap();
    fs::write(root.join(format!("{prefix}-setup.exe")), b"setup").unwrap();
    let invoke = |dry: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara-release"));
        command
            .args([
                "publish",
                "--repo",
                "Snowy7/ara",
                "--tag",
                "hub-v0.0.1-alpha",
                "--prerelease",
                "--artifacts",
            ])
            .arg(&root);
        if dry {
            command.arg("--dry-run");
        }
        command.output().unwrap()
    };
    let output = invoke(false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("rehearsal packages cannot be published")
    );
    let file = fs::File::options().write(true).open(&archive).unwrap();
    file.set_len(2 * 1024 * 1024 * 1024).unwrap();
    let output = invoke(true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("smaller than 2 GiB"));
    drop(file);
    fs::remove_file(archive).unwrap();
}

#[test]
fn dry_run_publishes_manifest_the_hub_verifies_without_gh_or_network() {
    for kind in ["hub", "engine"] {
        let root = testing::scratch(&format!("publish-{kind}"));
        let prefix = if kind == "hub" {
            "ara-hub-0.0.1-alpha-windows-x64"
        } else {
            "ara-0.0.1-alpha-windows-x64"
        };
        fs::create_dir(root.join(prefix)).unwrap();
        fs::write(root.join(prefix).join("hub.json"), br#"{"format":"ara.hub/1","version":"0.0.1-alpha","build":{"commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","generation":1}}"#).unwrap();
        fs::write(root.join(prefix).join("engine.json"), br#"{"format":"ara.engine/1","version":"0.0.1-alpha","sourceCommit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#).unwrap();
        fs::write(
            root.join(prefix).join("THIRD_PARTY_NOTICES.txt"),
            "fixture notices",
        )
        .unwrap();
        fs::write(
            root.join(format!("{prefix}.zip")),
            testing::engine_zip("0.0.1-alpha"),
        )
        .unwrap();
        fs::write(root.join(format!("{prefix}-setup.exe")), b"dummy installer").unwrap();
        fs::write(
            root.join(if kind == "hub" {
                "HUB_RELEASE_NOTES.md"
            } else {
                "RELEASE_NOTES.md"
            }),
            "Fixture release notes",
        )
        .unwrap();
        let key = root.join("synthetic-test-key");
        fs::write(&key, testing::test_seed()).unwrap();
        let tag = format!("{kind}-v0.0.1-alpha");
        let invoke = || {
            Command::new(env!("CARGO_BIN_EXE_ara-release"))
                .args([
                    "publish",
                    "--repo",
                    "Snowy7/ara",
                    "--tag",
                    &tag,
                    "--prerelease",
                    "--dry-run",
                    "--key",
                ])
                .arg(&key)
                .arg("--artifacts")
                .arg(&root)
                .output()
                .unwrap()
        };
        let output = invoke();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let folder = root.join("published").join(&tag);
        let manifest = fs::read(folder.join("release.json")).unwrap();
        let signature = fs::read(folder.join("release.json.sig")).unwrap();
        let release = verify(&manifest, &signature, &[testing::test_public_key()]).unwrap();
        assert_eq!(release.manifest().kind.name(), kind);
        assert!(release.manifest().notes.contains("Authenticode"));
        assert!(release.manifest().assets.iter().all(|a| {
            a.url
                .starts_with("https://github.com/Snowy7/ara/releases/download/")
        }));
        assert!(
            invoke().status.success(),
            "an identical reviewed folder is reusable"
        );
        fs::write(folder.join(&release.manifest().assets[0].name), b"tampered").unwrap();
        assert!(
            !invoke().status.success(),
            "a changed reviewed artifact cannot be published"
        );
        fs::remove_file(key).unwrap();
    }
}
