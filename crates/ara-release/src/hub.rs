use crate::{
    archive,
    support::{self, copy, run},
};
use anyhow::{Result, ensure};
use std::{fs, path::Path, process::Command};

/// Package the standalone GUI with an independently versioned bootstrap service.
pub fn package(root: &Path, version: &str) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "Hub packaging requires Windows x64"
    );
    ensure!(
        !ara_update::trusted_keys().is_empty(),
        "embed a release public key before building a Hub package"
    );
    let dist = support::dist(root);
    fs::create_dir_all(dist.join("logs"))?;
    let name = format!("ara-hub-{version}-windows-x64");
    let output = dist.join(&name);
    ensure!(!output.exists(), "{} already exists", output.display());
    run(
        Command::new("cargo")
            .current_dir(root)
            .env("CARGO_BUILD_JOBS", "4")
            .args([
                "build",
                "--release",
                "-p",
                "ara-hub",
                "--bins",
                "-p",
                "ara",
                "--bin",
                "ara",
                "-p",
                "ara-setup",
                "--bin",
                "ara-setup",
            ]),
        &dist.join("logs/hub-release-build.log"),
    )?;
    let stage = dist.join(format!(".stage-{name}-{}", std::process::id()));
    fs::create_dir(&stage)?;
    for binary in ["ara-hub", "ara-hub-updater"] {
        copy(
            &root.join(format!("target/release/{binary}.exe")),
            &stage.join(format!("{binary}.exe")),
        )?;
    }
    copy(
        &root.join("target/release/ara.exe"),
        &stage.join("bootstrap/ara.exe"),
    )?;
    support::json(
        &stage.join("bootstrap/service.json"),
        &serde_json::json!({"version":version,"build":ara_update::sha256_file(&stage.join("bootstrap/ara.exe"))?}),
    )?;
    for entry in fs::read_dir(root.join("target/release"))? {
        let file = entry?.path();
        if file
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            let name = file
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("DLL filename"))?;
            copy(&file, &stage.join(name))?;
            copy(&file, &stage.join("bootstrap").join(name))?;
        }
    }
    support::copy_tree(&root.join("assets/brand"), &stage.join("assets/brand"))?;
    copy(&root.join("release/LICENSE"), &stage.join("LICENSE"))?;
    copy(&root.join("release/paths.ps1"), &stage.join("paths.ps1"))?;
    fs::write(
        stage.join("install.ps1"),
        include_str!("../hub-install.ps1"),
    )?;
    support::json(
        &stage.join("hub.json"),
        &serde_json::json!({"format":"ara.hub/1","version":version}),
    )?;
    crate::notices::generate(root, &stage, &dist)?;
    fs::rename(stage, &output)?;
    let zip = dist.join(format!("{name}.zip"));
    archive::zip(&output, &zip)?;
    fs::write(
        dist.join(format!("{name}-setup.exe")),
        ara_export_installer::append(
            &fs::read(root.join("target/release/ara-setup.exe"))?,
            &fs::read(zip)?,
        )?,
    )?;
    fs::write(
        dist.join("HUB_RELEASE_NOTES.md"),
        format!(
            "# Ara Hub {version}\n\nDownload and run the per-user installer. Browse and install engine versions in Installs. Stable excludes GitHub prereleases; Preview includes them. Signed manifests and SHA-256 protect updates.\n\nNo Authenticode code signing yet. Windows SmartScreen warns when running the installer.\n"
        ),
    )?;
    println!("{}", output.display());
    Ok(())
}
