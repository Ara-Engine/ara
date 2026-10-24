//! Real game templates are pinned ZIPs containing the player and its cooked resources.
use anyhow::{Context, Result, ensure};
use ara_asset::{CacheBackend, Digest, FileCache};
use ara_pack::Input;
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) fn inputs(project: &Path, worker: &Path) -> Result<Vec<Input>> {
    let mut scene = ara_play_host::package_scene(project, worker).map_err(anyhow::Error::msg)?;
    ensure!(
        scene.scripts.is_none(),
        "exported C# modules require a shipping script host"
    );
    let cache = FileCache::at(&scene.cache)?;
    let mut generations = BTreeSet::new();
    let mut inputs = Vec::new();
    for (asset, value) in &scene.assets {
        let digest = Digest(ara_play::unhex(value).map_err(anyhow::Error::msg)?);
        if !generations.insert(digest) {
            continue;
        }
        let cached = cache
            .load(digest)?
            .with_context(|| format!("missing cooked asset {asset}"))?;
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&cached.output, &mut bytes)?;
        inputs.push(Input {
            path: format!(".ara/cooked/{digest}.cbor"),
            bytes,
        });
    }
    // A deployed player must never depend on the author's cache or navigation path.
    scene.cache.clear();
    scene.nav_cache = None;
    inputs.push(Input {
        path: ".ara/scene.cbor".into(),
        bytes: scene.encode().map_err(anyhow::Error::msg)?,
    });
    Ok(inputs)
}

/// Resolves an entire runtime cohort from the already hash-verified template ZIP.
/// Conformance templates retain their existing single-executable contract.
pub(crate) fn unpack_template(
    bundle: &Path,
    destination: &Path,
    executable: &str,
) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(File::open(bundle)?)
        .context("runtime template must be a ZIP containing ara-player and resources")?;
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .context("runtime template contains an unsafe path")?;
        let name = relative.to_string_lossy().replace('\\', "/");
        ara_pack::validate_path(name.trim_end_matches('/'))?;
        ensure!(
            names.insert(name.to_ascii_lowercase()),
            "duplicate runtime template path {name}"
        );
        ensure!(
            name == executable
                || name == "engine.json"
                || name == "SDL3.dll"
                || (entry.is_dir() && matches!(name.as_str(), "resources" | "dotnet"))
                || name.starts_with("resources/")
                || name.starts_with("dotnet/"),
            "unexpected runtime template file {name}"
        );
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "runtime template cannot contain symbolic links"
        );
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        ensure!(
            entry.size() <= 2 * 1024 * 1024 * 1024,
            "runtime resource exceeds 2 GiB"
        );
        std::fs::create_dir_all(output.parent().context("runtime resource parent")?)?;
        std::io::copy(&mut entry, &mut File::create(output)?)?;
    }
    for package in [
        "ara-render",
        "ara-render-fidelity",
        "ara-runtime-render",
        "ara-render2d",
        "ara-landscape",
        "ara-vfx-gpu",
    ] {
        ensure!(
            destination.join("resources").join(package).is_dir(),
            "runtime template lacks cooked {package} resources"
        );
    }
    let player = destination.join(executable);
    ensure!(player.is_file(), "runtime template lacks {executable}");
    Ok(player)
}
