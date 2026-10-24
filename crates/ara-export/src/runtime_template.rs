//! Packages a prebuilt player with exactly the resources named by current pipeline manifests.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

/// Builds a deterministic runtime-template ZIP for a subsequent hash pin. `engine`
/// is an installed engine folder or an explicit checkout; Rust is never compiled.
/// Only current cooked cohorts are included, so stale cooks cannot enter the player.
pub fn build_runtime_template(player: &Path, engine: &Path, output: &Path) -> Result<()> {
    let player_name = player
        .file_name()
        .context("player executable filename")?
        .to_str()
        .context("player executable name is not UTF-8")?;
    ensure!(
        matches!(player_name, "ara-player" | "ara-player.exe"),
        "expected a prebuilt ara-player executable"
    );
    let mut files = BTreeMap::new();
    files.insert(player_name.to_owned(), player.to_owned());
    for package in [
        "ara-render",
        "ara-render-fidelity",
        "ara-runtime-render",
        "ara-render2d",
        "ara-landscape",
        "ara-vfx-gpu",
    ] {
        let installed = engine.join("resources").join(package);
        let root = if installed.is_dir() {
            installed
        } else {
            engine.join("crates").join(package).join("cooked")
        };
        ensure!(root.is_dir(), "missing cooked {package} resources");
        resources(&root, &root, &format!("resources/{package}"), &mut files)?;
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .last_modified_time(zip::DateTime::default())
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, path) in files {
        zip.start_file(name, options)?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut zip)?;
    }
    zip.start_file("engine.json", options)?;
    let marker = serde_json::json!({"format":"ara.player.runtime-template/1"});
    zip.write_all(ara_json::to_canonical_string(&ara_json::sort_keys(&marker)).as_bytes())?;
    zip.finish()?;
    Ok(())
}

fn resources(
    root: &Path,
    directory: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                resources(root, &path, prefix, files)?;
            }
            continue;
        }
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        ara_pack::validate_path(&relative)?;
        files.insert(format!("{prefix}/{relative}"), path.to_owned());
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let Some(stages) = value.get("stages").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for stage in stages {
            for (field, reflection) in [("spirv", "spirv.json"), ("dxil", "dxil.json")] {
                let Some(name) = stage.get(field).and_then(serde_json::Value::as_str) else {
                    continue;
                };
                ara_pack::validate_path(name)?;
                let artifact = path.parent().context("pipeline directory")?.join(name);
                ensure!(
                    artifact.is_file(),
                    "missing cooked artifact {}",
                    artifact.display()
                );
                for file in [artifact.to_owned(), artifact.with_extension(reflection)] {
                    if file.is_file() {
                        let relative = file
                            .strip_prefix(root)?
                            .to_string_lossy()
                            .replace('\\', "/");
                        files.insert(format!("{prefix}/{relative}"), file);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn runtime_bundle_is_stable_and_excludes_unused_cooked_cohorts() {
        let scratch = tempfile::tempdir().unwrap();
        let player = scratch.path().join("ara-player.exe");
        std::fs::write(&player, b"prebuilt player").unwrap();
        let key = "a".repeat(64);
        let stale = "b".repeat(64);
        for package in [
            "ara-render",
            "ara-render-fidelity",
            "ara-runtime-render",
            "ara-render2d",
            "ara-landscape",
            "ara-vfx-gpu",
        ] {
            let root = scratch.path().join("crates").join(package).join("cooked");
            for cohort in [&key, &stale] {
                std::fs::create_dir_all(root.join(cohort)).unwrap();
            }
            std::fs::write(root.join(&key).join("main.spv"), b"current shader").unwrap();
            std::fs::write(root.join(&stale).join("main.spv"), b"obsolete shader").unwrap();
            let value = serde_json::json!({"stages":[{"spirv":format!("{key}/main.spv")} ]});
            std::fs::write(
                root.join("main.json"),
                ara_json::to_canonical_string(&ara_json::sort_keys(&value)),
            )
            .unwrap();
        }
        let first = scratch.path().join("first.zip");
        let second = scratch.path().join("second.zip");
        build_runtime_template(&player, scratch.path(), &first).unwrap();
        build_runtime_template(&player, scratch.path(), &second).unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&first).unwrap()).unwrap();
        assert!(
            archive
                .by_name(&format!("resources/ara-render/{key}/main.spv"))
                .is_ok()
        );
        assert!(
            archive
                .by_name(&format!("resources/ara-render/{stale}/main.spv"))
                .is_err()
        );
        let unpacked = scratch.path().join("game");
        let executable =
            crate::runtime::unpack_template(&first, &unpacked, "ara-player.exe").unwrap();
        assert_eq!(std::fs::read(executable).unwrap(), b"prebuilt player");
    }
}
