//! Real game templates are pinned ZIPs containing the player and its cooked resources.
use anyhow::{Context, Result, ensure};
use ara_asset::{CacheBackend, Digest, FileCache};
use ara_pack::Input;
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) fn inputs(
    project: &Path,
    worker: &Path,
    scripts: Option<ara_play::ScriptScene>,
    report: impl FnMut(ara_play_host::assets::CookProgress) -> Result<(), String>,
) -> Result<Vec<Input>> {
    let mut scene = ara_play_host::package_scene_for_export(project, worker, scripts, report)
        .map_err(anyhow::Error::msg)?;
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
    validate_portable_scene(&scene)?;
    inputs.push(Input {
        path: ".ara/scene.cbor".into(),
        bytes: scene.encode().map_err(anyhow::Error::msg)?,
    });
    Ok(inputs)
}

/// The pack keeps authored logical paths and compiled gameplay only. Filesystem
/// roots belong to authoring/import workers and are never part of player discovery.
fn validate_portable_scene(scene: &ara_play::PlayScene) -> Result<()> {
    for document in &scene.documents {
        ara_pack::validate_path(&document.path)?;
    }
    for path in scene.imports.values() {
        ara_pack::validate_path(path)?;
    }
    if let Some(scripts) = &scene.scripts {
        let ara_play::ScriptHost::NativeAot { manifest } = &scripts.host else {
            anyhow::bail!("ARA0920: an export cannot include an author-machine script host");
        };
        ara_pack::validate_path(manifest)?;
        ensure!(
            scripts.host_dir.is_empty()
                && scripts.compile_server.is_none()
                && scripts.modules.is_empty(),
            "ARA0920: an exported script host must not contain SDK/compiler/source paths"
        );
    }
    Ok(())
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
                || name == "THIRD_PARTY_NOTICES.txt"
                || name == crate::macos::ICON
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
    crate::runtime_template::inventory(&player, destination).context(
        "ARA-EXP-PREFLIGHT-001: pinned runtime template is missing or has corrupt cooked resources",
    )?;
    Ok(player)
}
