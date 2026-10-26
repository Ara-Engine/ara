use crate::{
    archive,
    support::{self, copy, run},
};
use anyhow::{Result, ensure};
use std::{fs, path::Path, process::Command};

/// Package the standalone GUI with an independently versioned bootstrap service.
pub fn package(root: &Path, version: &str) -> Result<()> {
    package_inner(root, version, true)
}

/// Called only after engine packaging built the complete package/feature union.
pub(crate) fn package_prebuilt(root: &Path, version: &str) -> Result<()> {
    package_inner(root, version, false)
}

fn package_inner(root: &Path, version: &str, build_required: bool) -> Result<()> {
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
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "line-tables-only")
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
        ]);
    if cfg!(feature = "rehearsal") {
        build.args(["--features", "ara-hub/rehearsal"]);
    }
    if build_required {
        run(&mut build, &dist.join("logs/hub-release-build.log"))?;
    }
    let stage = dist.join(format!(".stage-{name}-{}", std::process::id()));
    fs::create_dir(&stage)?;
    for binary in ["ara-hub", "ara-hub-updater"] {
        copy(
            &support::profile(root, "release").join(format!("{binary}.exe")),
            &stage.join(format!("{binary}.exe")),
        )?;
    }
    copy(
        &support::profile(root, "release").join("ara.exe"),
        &stage.join("bootstrap/ara.exe"),
    )?;
    support::json(
        &stage.join("bootstrap/service.json"),
        &serde_json::json!({"version":version,"build":ara_update::sha256_file(&stage.join("bootstrap/ara.exe"))?}),
    )?;
    for entry in fs::read_dir(support::profile(root, "release"))? {
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
    copy(
        &root.join("release/hub-installation.ps1"),
        &stage.join("hub-installation.ps1"),
    )?;
    fs::write(
        stage.join("install.ps1"),
        include_str!("../hub-install.ps1"),
    )?;
    support::json(&stage.join("hub.json"), &marker(root, version)?)?;
    verify_binary(root, &stage, version, &dist)?;
    crate::notices::generate(root, &stage, &dist)?;
    fs::rename(stage, &output)?;
    let zip = dist.join(format!("{name}.zip"));
    archive::zip(&output, &zip)?;
    fs::write(
        dist.join(format!("{name}-setup.exe")),
        ara_export_installer::append(
            &fs::read(support::profile(root, "release").join("ara-setup.exe"))?,
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

/// Both distributions carry the same ordered source identity, not just semver.
pub(crate) fn marker(root: &Path, version: &str) -> Result<serde_json::Value> {
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git").current_dir(root).args(args).output()?;
        ensure!(
            output.status.success(),
            "cannot identify release source commit"
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    ensure!(
        git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty(),
        "commit source changes before packaging a build-identified Hub"
    );
    Ok(
        serde_json::json!({"format":"ara.hub/1","version":version,"rehearsal":cfg!(feature="rehearsal"),"build":{
            "commit":git(&["rev-parse","HEAD"])?,"generation":git(&["rev-list","--count","HEAD"])?.parse::<u64>()?
        }}),
    )
}

/// Refuse stale compiler output before it can become a signed release package.
pub(crate) fn verify_binary(root: &Path, stage: &Path, version: &str, dist: &Path) -> Result<()> {
    let report = dist.join("logs/hub-build-info.json");
    let mut command = Command::new(stage.join("ara-hub.exe"));
    command
        .arg("--build-info")
        .arg(&report)
        .env("ARA_CONFIG_DIR", dist.join("package-config"))
        .env("ARA_CRASH_DIR", dist.join("package-crashes"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    run(&mut command, &dist.join("logs/hub-build-info.log"))?;
    let info: serde_json::Value = serde_json::from_slice(&fs::read(report)?)?;
    ensure!(
        info["build"] == marker(root, version)?["build"] && info["version"] == version,
        "ara-hub.exe is from a different source build; rebuild before packaging"
    );
    Ok(())
}
