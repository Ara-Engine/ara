use crate::{preset::TemplatePin, tools};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Resolves a pinned runtime into an immutable content-addressed local cache.
/// Every hit is rehashed. Cache corruption never silently falls back to a new pin.
pub fn resolve(pin: &TemplatePin, project: &Path, cache: &Path) -> Result<PathBuf> {
    resolve_artifact(
        &crate::preset::ArtifactPin {
            hash: pin.hash.clone(),
            source: pin.source.clone(),
        },
        project,
        cache,
    )
}

/// Resolves a tooling/symbol artifact with the same content-addressed checks as a runtime.
pub fn resolve_artifact(
    pin: &crate::preset::ArtifactPin,
    project: &Path,
    cache: &Path,
) -> Result<PathBuf> {
    pin.validate()?;
    let root = cache.join("templates");
    std::fs::create_dir_all(&root)?;
    let target = root.join(&pin.hash);
    if target.is_file() {
        verify(&target, &pin.hash)?;
        return Ok(target);
    }
    let temp = root.join(format!("{}.{}.partial", pin.hash, std::process::id()));
    let result = (|| {
        if pin.source.starts_with("https://") {
            tools::run(
                Command::new("curl")
                    .args([
                        "--fail",
                        "--location",
                        "--proto",
                        "=https",
                        "--tlsv1.2",
                        "--output",
                    ])
                    .arg(&temp)
                    .arg(&pin.source),
                &cache.join("template-download.log"),
                Duration::from_secs(120),
            )?;
        } else {
            let source = Path::new(&pin.source);
            tools::copy(
                &if source.is_absolute() {
                    source.to_path_buf()
                } else {
                    project.join(source)
                },
                &temp,
            )?;
        }
        verify(&temp, &pin.hash)?;
        match std::fs::rename(&temp, &target) {
            Ok(()) => {}
            Err(_) if target.is_file() => verify(&target, &pin.hash)?,
            Err(e) => return Err(e.into()),
        }
        Ok(target.clone())
    })();
    if temp.is_file() {
        std::fs::remove_file(&temp).context("removing partial template")?;
    }
    result
}

fn verify(path: &Path, hash: &str) -> Result<()> {
    ensure!(
        blake3::hash(&std::fs::read(path)?).to_hex().as_str() == hash,
        "ARA-EXP-TPL-004: template hash mismatch at {}",
        path.display()
    );
    Ok(())
}
