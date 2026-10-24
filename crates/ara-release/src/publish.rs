//! Signed local artifacts first; gh is used only for an explicit non-dry publish.
use anyhow::{Context, Result, ensure};
use ara_update::{Asset, Channel, Kind, Manifest, canonical_manifest, sha256_file};
use std::{fs, io::Write, path::Path, process::Command};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ReleaseKind {
    Hub,
    Engine,
}

fn key(path: Option<&Path>) -> Result<String> {
    let path = path
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ARA_RELEASE_SIGNING_KEY").map(Into::into))
        .context("set ARA_RELEASE_SIGNING_KEY to the private key path or pass --key PATH")?;
    fs::read_to_string(path)
        .context("read release signing key (expected a path to a hexadecimal seed)")
}
pub fn keygen(root: &Path, output: &Path) -> Result<()> {
    let parent = output
        .parent()
        .context("private key needs a parent directory")?;
    fs::create_dir_all(parent)?;
    ensure!(
        !parent.canonicalize()?.starts_with(root.canonicalize()?),
        "private signing keys must be outside the repository"
    );
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(|_| anyhow::anyhow!("OS random generator failed"))?;
    let text = hex::encode(seed);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(output)
        .context("create private key (refuses to overwrite)")?;
    #[cfg(windows)]
    {
        // Remove inherited access; grant only the current Windows user. No secret on command line.
        let domain = std::env::var("USERDOMAIN").context("USERDOMAIN is unavailable")?;
        let user = std::env::var("USERNAME").context("USERNAME is unavailable")?;
        let status = Command::new("icacls.exe")
            .arg(output)
            .args([
                "/inheritance:r",
                "/grant:r",
                &format!("{domain}\\{user}:(F)"),
            ])
            .output()?;
        ensure!(
            status.status.success(),
            "could not restrict private key ACL; no key material was written"
        );
    }
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    let (_, public) = ara_update::sign(b"", &text)?;
    println!(
        "Public key: {public}\nEmbed in crates/ara-update/public-keys.txt before building releases.\nPrivate key saved outside the repository at {}",
        output.display()
    );
    Ok(())
}
pub fn sign_file(path: &Path, key_path: Option<&Path>) -> Result<()> {
    let bytes = fs::read(path)?;
    let (signature, public) = ara_update::sign(&bytes, &key(key_path)?)?;
    ara_update::verify(&bytes, signature.as_bytes(), &[public])?;
    fs::write(path.with_file_name("release.json.sig"), signature)?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub fn publish(
    root: &Path,
    repo: &str,
    tag: &str,
    prerelease: bool,
    dry_run: bool,
    key_path: Option<&Path>,
    artifacts: Option<&Path>,
) -> Result<()> {
    let mut config = ara_update::FeedConfig::default();
    config.repo = repo.into();
    config.validate()?;
    let (kind, version) = if let Some(v) = tag.strip_prefix("hub-v") {
        (Kind::Hub, v)
    } else if let Some(v) = tag.strip_prefix("engine-v") {
        (Kind::Engine, v)
    } else {
        anyhow::bail!("tag must be hub-v<semver> or engine-v<semver>");
    };
    ara_update::version(version)?;
    let dist = artifacts
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::support::dist(root));
    let prefix = match kind {
        Kind::Engine => crate::support::name(version),
        Kind::Hub => format!("ara-hub-{version}-windows-x64"),
    };
    let archive = dist.join(format!("{prefix}.zip"));
    let setup = dist.join(format!("{prefix}-setup.exe"));
    ensure!(
        archive.is_file() && setup.is_file(),
        "package {} {version} first; missing ZIP or installer in {}",
        kind.name(),
        dist.display()
    );
    let notes_path = dist.join(if kind == Kind::Hub {
        "HUB_RELEASE_NOTES.md"
    } else {
        "RELEASE_NOTES.md"
    });
    let mut notes = fs::read_to_string(&notes_path).context("package release notes are missing")?;
    if !notes.contains("Authenticode") {
        notes.push_str("\nNo Authenticode code signing yet. Windows SmartScreen warns when running the installer.\n");
    }
    let mut files = vec![archive, setup];
    let notices = dist.join(&prefix).join("THIRD_PARTY_NOTICES.txt");
    let notices_md = dist.join(&prefix).join("THIRD_PARTY_NOTICES.md");
    if notices.is_file() {
        files.push(notices);
    } else if notices_md.is_file() {
        files.push(notices_md);
    } else {
        anyhow::bail!("packaged third-party notices are missing");
    }
    let assets = files
        .iter()
        .map(|path| -> Result<Asset> {
            let name = path
                .file_name()
                .context("artifact filename")?
                .to_string_lossy()
                .into_owned();
            Ok(Asset {
                url: format!("https://github.com/{repo}/releases/download/{tag}/{name}"),
                name,
                size: fs::metadata(path)?.len(),
                sha256: sha256_file(path)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = Manifest {
        format: "ara.release/1".into(),
        kind,
        version: version.into(),
        channel: if prerelease {
            Channel::Preview
        } else {
            Channel::Stable
        },
        min_hub_version: if kind == Kind::Engine {
            env!("CARGO_PKG_VERSION")
        } else {
            "0.0.1-alpha"
        }
        .into(),
        notes: notes.clone(),
        platform: "windows-x64".into(),
        assets,
    };
    let bytes = canonical_manifest(&manifest)?;
    let (signature, public) = ara_update::sign(&bytes, &key(key_path)?)?;
    // A publisher must be accepted by this build's trust roots; dry runs can bootstrap a new key.
    ara_update::verify(&bytes, signature.as_bytes(), std::slice::from_ref(&public))?;
    if !dry_run {
        ara_update::verify(&bytes, signature.as_bytes(), &ara_update::trusted_keys()).context(
            "release signing key is not embedded; rebuild Hub/release tool with the public key",
        )?;
    }
    let folder = dist.join("published").join(tag);
    if !folder.exists() {
        fs::create_dir_all(&folder)?;
        for file in &files {
            fs::copy(file, folder.join(file.file_name().context("filename")?))?;
        }
        fs::write(folder.join("release.json"), &bytes)?;
        fs::write(folder.join("release.json.sig"), &signature)?;
        fs::write(folder.join("RELEASE_NOTES.md"), &notes)?;
    }
    let uploads = verify_folder(&folder, &manifest, &bytes, &signature, &notes)?;
    // Public metadata only; useful for independently checking a dry run.
    println!(
        "Release folder: {}\nSigning public key: {public}",
        folder.display()
    );
    if dry_run {
        return Ok(());
    }
    let gh = Command::new("gh").arg("--version").output().context("GitHub CLI (gh) is required only for publishing; install it and run gh auth login, or use --dry-run")?;
    ensure!(gh.status.success(), "GitHub CLI gh --version failed");
    let mut create = Command::new("gh");
    create
        .args([
            "release",
            "create",
            tag,
            "--repo",
            repo,
            "--title",
            &format!("Ara {} {version}", kind.name()),
            "--notes-file",
        ])
        .arg(folder.join("RELEASE_NOTES.md"));
    if prerelease {
        create.arg("--prerelease");
    }
    // Create as draft: consumers never observe a release before its signature/assets finish uploading.
    create.arg("--draft");
    let created = create.status()?;
    ensure!(
        created.success(),
        "gh release create failed; local release retained"
    );
    let mut upload = Command::new("gh");
    upload.args(["release", "upload", tag, "--repo", repo]);
    for path in uploads {
        upload.arg(path);
    }
    ensure!(
        upload.status()?.success(),
        "gh release upload failed; draft retained for retry"
    );
    ensure!(
        Command::new("gh")
            .args(["release", "edit", tag, "--repo", repo, "--draft=false"])
            .status()?
            .success(),
        "assets uploaded but draft could not be published"
    );
    Ok(())
}

/// Reuse the exact reviewed dry-run folder; never replace it or upload extra files.
fn verify_folder(
    folder: &Path,
    manifest: &Manifest,
    bytes: &[u8],
    signature: &str,
    notes: &str,
) -> Result<Vec<std::path::PathBuf>> {
    ensure!(
        fs::read(folder.join("release.json"))? == bytes
            && fs::read(folder.join("release.json.sig"))? == signature.as_bytes()
            && fs::read(folder.join("RELEASE_NOTES.md"))? == notes.as_bytes(),
        "existing release folder differs from this manifest/signing key/notes; preserve it and choose a fresh artifact directory"
    );
    let mut uploads = Vec::new();
    for asset in &manifest.assets {
        let path = folder.join(&asset.name);
        ensure!(
            fs::metadata(&path)?.len() == asset.size && sha256_file(&path)? == asset.sha256,
            "reviewed artifact changed: {}",
            path.display()
        );
        uploads.push(path);
    }
    uploads.extend(
        ["release.json", "release.json.sig", "RELEASE_NOTES.md"].map(|name| folder.join(name)),
    );
    let actual = crate::support::files(folder)?;
    ensure!(
        actual.len() == uploads.len() && actual.iter().all(|path| uploads.contains(path)),
        "release folder contains unexpected files; nothing will be uploaded"
    );
    Ok(uploads)
}
