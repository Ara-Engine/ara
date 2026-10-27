//! Generic packages and a plain, content-addressed upload manifest.
use crate::{ExportPreset, Platform, preset::PackageFormat, tools};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Exact bytes published in the plain build folder or a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    /// BLAKE3 of the complete file.
    pub hash: String,
    /// File length in bytes.
    pub bytes: u64,
    /// Unix execute permission needed after download/extraction.
    pub executable: bool,
}

/// Service-neutral upload contract, alongside `build/`, packages and symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionManifest {
    /// Always `ara.distribution/1`.
    pub format: String,
    /// Identity selecting this build's symbol archive.
    pub build_id: String,
    /// Destination OS.
    pub platform: String,
    /// Native player entry point, relative to the build folder.
    pub entry_point: String,
    /// Files under `build/`, with portable relative paths.
    pub files: BTreeMap<String, FileIdentity>,
    /// Package artifacts, relative to this manifest.
    pub packages: BTreeMap<String, FileIdentity>,
    /// Private symbol ZIP; deliberately excluded from all player packages.
    pub symbols: String,
    /// Actual signing state; unsigned Windows packages remain usable.
    pub signing: String,
}

pub(crate) fn files(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    fn walk(
        root: &Path,
        path: &Path,
        out: &mut BTreeMap<String, PathBuf>,
        seen: &mut BTreeSet<String>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            ensure!(
                !ty.is_symlink(),
                "ARA-EXP-DIST-002: distribution symlinks are not supported"
            );
            if ty.is_dir() {
                walk(root, &entry.path(), out, seen)?;
            } else {
                ensure!(
                    ty.is_file(),
                    "ARA-EXP-DIST-002: distribution requires regular files"
                );
                let name = entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 distribution path")?
                    .replace('\\', "/");
                ara_pack::validate_path(&name)?;
                ensure!(
                    seen.insert(name.to_lowercase()),
                    "ARA-EXP-DIST-002: case-colliding distribution paths"
                );
                out.insert(name, entry.path());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out, &mut BTreeSet::new())?;
    Ok(out)
}
fn executable(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut magic = [0u8; 4];
    let len = std::fs::File::open(path)?.read(&mut magic)?;
    Ok((len == 4 && &magic == b"\x7fELF") || magic.starts_with(b"#!"))
}
pub(crate) fn identities(root: &Path) -> Result<BTreeMap<String, FileIdentity>> {
    files(root)?
        .into_iter()
        .map(|(name, path)| Ok((name, identity(&path)?)))
        .collect()
}
fn identity(path: &Path) -> Result<FileIdentity> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = blake3::Hasher::new();
    hash.update_reader(&mut file)?;
    Ok(FileIdentity {
        hash: hash.finalize().to_hex().to_string(),
        bytes: file.metadata()?.len(),
        executable: executable(path)?,
    })
}

/// Writes sorted files with fixed ZIP metadata and proper executable permissions.
pub fn portable_zip(build: &Path, output: &Path) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(output)?);
    for (name, path) in files(build)? {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .last_modified_time(zip::DateTime::default())
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(if executable(&path)? { 0o755 } else { 0o644 }),
        )?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut zip)?;
    }
    zip.finish()?.sync_all()?;
    Ok(())
}

/// Writes a deterministic tar.gz with uid/gid/mtime zero and executable ELF files.
pub fn tar_gz(build: &Path, output: &Path) -> Result<()> {
    let gzip = flate2::GzBuilder::new().mtime(0).write(
        std::fs::File::create(output)?,
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(gzip);
    for (name, path) in files(build)? {
        let mut header = tar::Header::new_gnu();
        header.set_size(std::fs::metadata(&path)?.len());
        header.set_mode(if executable(&path)? { 0o755 } else { 0o644 });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        tar.append_data(&mut header, name, std::fs::File::open(path)?)?;
    }
    tar.into_inner()?.finish()?.sync_all()?;
    Ok(())
}

/// Builds a type-2 AppImage on any host, using a pinned runtime and Rust SquashFS.
pub fn appimage(build: &Path, runtime: &Path, output: &Path) -> Result<()> {
    let bytes = std::fs::read(runtime)?;
    ensure!(
        bytes.starts_with(b"\x7fELF")
            && bytes.get(8..11) == Some(b"AI\x02")
            && bytes.get(18..20) == Some(&62u16.to_le_bytes()),
        "ARA-EXP-APPIMAGE-002: expected an x86_64 type-2 AppImage runtime"
    );
    let mut fs = backhand::FilesystemWriter::default();
    fs.set_time(0);
    fs.set_root_mode(0o755);
    fs.set_compressor(backhand::FilesystemCompressor::new(
        backhand::compression::Compressor::Gzip,
        None,
    )?);
    for (name, path) in files(build)? {
        if let Some(parent) = Path::new(&name)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            fs.push_dir_all(parent, backhand::NodeHeader::new(0o755, 0, 0, 0))?;
        }
        let mode = if executable(&path)? { 0o755 } else { 0o644 };
        fs.push_file_from_path(path, name, backhand::NodeHeader::new(mode, 0, 0, 0))?;
    }
    fs.push_file(std::io::Cursor::new(b"#!/bin/sh\nHERE=\"$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\"\nexec \"$HERE/ara-player\" --root \"$HERE\" \"$@\"\n"), "AppRun", backhand::NodeHeader::new(0o755, 0, 0, 0))?;
    fs.push_file(std::io::Cursor::new(b"[Desktop Entry]\nType=Application\nName=Ara Game\nExec=AppRun\nIcon=ara-game\nCategories=Game;\n"), "ara-game.desktop", backhand::NodeHeader::new(0o644, 0, 0, 0))?;
    let icon = include_bytes!("../../../assets/brand/icons/game/ara-game.svg");
    for name in ["ara-game.svg", ".DirIcon"] {
        fs.push_file(
            std::io::Cursor::new(icon),
            name,
            backhand::NodeHeader::new(0o644, 0, 0, 0),
        )?;
    }
    let mut out = std::fs::File::create(output)?;
    out.write_all(&bytes)?;
    fs.write_with_offset(&mut out, bytes.len() as u64)?;
    out.sync_all()?;
    let reader = backhand::FilesystemReader::from_reader_with_offset(
        std::io::BufReader::new(std::fs::File::open(output)?),
        bytes.len() as u64,
    )?;
    let expected = identities(build)?;
    let mut verified = BTreeSet::new();
    for node in reader.files() {
        if let backhand::InnerNode::File(file) = &node.inner {
            let path = node
                .fullpath
                .to_str()
                .context("non-UTF8 AppImage path")?
                .replace('\\', "/");
            let name = path.trim_start_matches('/');
            let mut hash = blake3::Hasher::new();
            hash.update_reader(reader.file(file).reader_checked()?)?;
            if let Some(identity) = expected.get(name) {
                ensure!(
                    hash.finalize().to_hex().as_str() == identity.hash,
                    "ARA-EXP-APPIMAGE-003: embedded payload hash mismatch"
                );
                verified.insert(name.to_string());
            }
            if name == "AppRun" {
                ensure!(
                    node.header.permissions & 0o111 != 0,
                    "ARA-EXP-APPIMAGE-003: AppRun is not executable"
                );
            }
        }
    }
    ensure!(
        verified.len() == expected.len(),
        "ARA-EXP-APPIMAGE-003: embedded payload is incomplete"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Finds signtool using an explicit override, PATH, or the Windows SDK x64 tools.
pub fn signtool() -> Option<PathBuf> {
    std::env::var_os("ARA_SIGNTOOL")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| tools::which("signtool"))
        .or_else(|| {
            let root =
                PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Windows Kits/10/bin");
            let mut paths = std::fs::read_dir(root)
                .ok()?
                .filter_map(|e| Some(e.ok()?.path().join("x64/signtool.exe")))
                .filter(|p| p.is_file())
                .collect::<Vec<_>>();
            paths.sort();
            paths.pop()
        })
}
pub(crate) fn sign(path: &Path, preset: &ExportPreset, log: &Path) -> Result<()> {
    let Some(signing) = &preset.windows_signing else {
        return Ok(());
    };
    let tool = signtool().context(
        "ARA-EXP-SIGN-002: signtool missing; set ARA_SIGNTOOL or install Windows SDK signing tools",
    )?;
    let thumbprint = std::env::var(&signing.certificate_env)
        .context("ARA-EXP-SIGN-003: signing certificate environment variable is unset")?;
    ensure!(
        thumbprint.len() == 40 && thumbprint.bytes().all(|b| b.is_ascii_hexdigit()),
        "ARA-EXP-SIGN-003: signing certificate must be a SHA1 thumbprint"
    );
    let mut cmd = Command::new(&tool);
    cmd.args(["sign", "/fd", "SHA256", "/sha1", &thumbprint]);
    if let Some(url) = &signing.timestamp_url {
        cmd.args(["/tr", url, "/td", "SHA256"]);
    }
    tools::run(cmd.arg(path), log, Duration::from_secs(60))?;
    tools::run(
        Command::new(tool).args(["verify", "/pa", "/all"]).arg(path),
        &log.with_extension("verify.log"),
        Duration::from_secs(30),
    )?;
    Ok(())
}

pub(crate) fn package(
    p: &ExportPreset,
    project: &Path,
    cache: &Path,
    build: &Path,
    output: &Path,
    work: &Path,
) -> Result<Vec<String>> {
    let mut names = Vec::new();
    match p.packaging {
        PackageFormat::WindowsPortable | PackageFormat::WindowsInstaller => {
            portable_zip(build, &output.join("game.zip"))?;
            names.push("game.zip".into());
            if p.packaging == PackageFormat::WindowsInstaller {
                let pin = p
                    .installer_stub
                    .as_ref()
                    .context("ARA-EXP-INSTALL-005: pinned installer stub missing")?;
                let stub = crate::template::resolve_artifact(pin, project, cache)?;
                let bytes = ara_export_installer::append(
                    &std::fs::read(stub)?,
                    &std::fs::read(output.join("game.zip"))?,
                )?;
                std::fs::write(output.join("game-setup.exe"), bytes)?;
                sign(
                    &output.join("game-setup.exe"),
                    p,
                    &work.join("logs/sign-installer.log"),
                )?;
                ara_export_installer::list(&std::fs::read(output.join("game-setup.exe"))?)?;
                names.push("game-setup.exe".into());
            }
        }
        PackageFormat::LinuxTar => {
            tar_gz(build, &output.join("game.tar.gz"))?;
            names.push("game.tar.gz".into());
        }
        PackageFormat::LinuxAppimage => {
            let pin = p
                .appimage_runtime
                .as_ref()
                .context("ARA-EXP-APPIMAGE-001: pinned runtime missing")?;
            let runtime = crate::template::resolve_artifact(pin, project, cache)?;
            appimage(build, &runtime, &output.join("game.AppImage"))?;
            names.push("game.AppImage".into());
        }
        PackageFormat::Folder | PackageFormat::AndroidPlay => {}
    }
    if p.platform == Platform::Android {
        names.extend(["game.apk".into(), "game.aab".into()]);
    }
    Ok(names)
}
pub(crate) fn manifest(
    p: &ExportPreset,
    build_id: &str,
    output: &Path,
    packages: &[String],
    signing: &str,
) -> Result<()> {
    let manifest = DistributionManifest {
        format: "ara.distribution/1".into(),
        build_id: build_id.into(),
        platform: p.platform.name().into(),
        entry_point: if p.platform == Platform::Windows {
            "ara-player.exe"
        } else if p.platform == Platform::Linux {
            "ara-player"
        } else {
            "android.app.NativeActivity"
        }
        .into(),
        files: identities(&output.join("build"))?,
        packages: packages
            .iter()
            .map(|name| Ok((name.clone(), identity(&output.join(name))?)))
            .collect::<Result<_>>()?,
        symbols: format!("symbols/{build_id}.zip"),
        signing: signing.into(),
    };
    tools::write_json(&output.join("distribution.json"), &manifest)
}
