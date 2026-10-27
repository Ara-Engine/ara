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
/// A macOS (Mach-O) player gets the Metal artifacts instead of SPIR-V and DXIL, the
/// packages only Metal loads from disk, and the game icon as `AppIcon.icns`.
pub fn build_runtime_template(player: &Path, engine: &Path, output: &Path) -> Result<()> {
    let Inventory { files, icon } = inventory(player, engine)?;
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
    if let Some(icon) = icon {
        zip.start_file(crate::macos::ICON, options)?;
        zip.write_all(&icon)?;
    }
    zip.start_file("engine.json", options)?;
    let marker = serde_json::json!({"format":"ara.player.runtime-template/1"});
    zip.write_all(ara_json::to_canonical_string(&ara_json::sort_keys(&marker)).as_bytes())?;
    zip.finish()?;
    Ok(())
}

pub(crate) struct Inventory {
    pub files: BTreeMap<String, PathBuf>,
    pub icon: Option<Vec<u8>>,
}

/// The exact inputs read by packaging, also used by the read-only preflight.
pub(crate) fn inventory(player: &Path, engine: &Path) -> Result<Inventory> {
    let player_name = player
        .file_name()
        .context("player executable filename")?
        .to_str()
        .context("player executable name is not UTF-8")?;
    ensure!(
        matches!(player_name, "ara-player" | "ara-player.exe"),
        "expected a prebuilt ara-player executable"
    );
    let metal = mach_o(player)?;
    let mut files = BTreeMap::new();
    files.insert(player_name.to_owned(), player.to_owned());
    if let Some(notices) = [
        engine.join("THIRD_PARTY_NOTICES.txt"),
        engine.join("THIRD_PARTY_NOTICES.md"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    {
        files.insert("THIRD_PARTY_NOTICES.txt".into(), notices);
    }
    let packages: &[&str] = if metal {
        &[
            "ara-render",
            "ara-render-fidelity",
            "ara-runtime-render",
            "ara-render2d",
            "ara-landscape",
            "ara-vfx-gpu",
            "ara-texture-gpu",
            "ara-gpu",
            "ara-editor-gpu",
        ]
    } else {
        &[
            "ara-render",
            "ara-render-fidelity",
            "ara-runtime-render",
            "ara-render2d",
            "ara-landscape",
            "ara-vfx-gpu",
        ]
    };
    for package in packages {
        let installed = engine.join("resources").join(package);
        let root = if installed.is_dir() {
            installed
        } else {
            engine.join("crates").join(package).join("cooked")
        };
        ensure!(root.is_dir(), "missing cooked {package} resources");
        let manifests = resources(
            &root,
            &root,
            &format!("resources/{package}"),
            metal,
            &mut files,
        )?;
        ensure!(manifests > 0, "missing cooked {package} pipeline manifests");
    }
    let icon = if metal {
        Some(if engine.join(crate::macos::ICON).is_file() {
            std::fs::read(engine.join(crate::macos::ICON))?
        } else {
            crate::macos::icns(&engine.join("assets/brand/icons/game"), "ara-game")?
        })
    } else {
        None
    };
    Ok(Inventory { files, icon })
}

/// Whether `player` is a Mach-O executable, the macOS player.
fn mach_o(player: &Path) -> Result<bool> {
    use std::io::Read;
    let mut magic = [0; 4];
    std::fs::File::open(player)?.read_exact(&mut magic)?;
    Ok(matches!(
        magic,
        [0xcf, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe]
    ))
}

/// Adds runtime artifacts named by manifests: SPIR-V/DXIL, or Metal libraries and
/// their required converter reflection. Slang reflection is an offline cook cache.
fn resources(
    root: &Path,
    directory: &Path,
    prefix: &str,
    metal: bool,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<usize> {
    let mut manifests = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                manifests += resources(root, &path, prefix, metal, files)?;
            }
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let Some(stages) = value.get("stages").and_then(serde_json::Value::as_array) else {
            continue;
        };
        ensure!(
            !stages.is_empty(),
            "empty cooked pipeline {}",
            path.display()
        );
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        ara_pack::validate_path(&relative)?;
        files.insert(format!("{prefix}/{relative}"), path.to_owned());
        manifests += 1;
        for stage in stages {
            if metal {
                let cooked = stage.get("metal").with_context(|| {
                    format!(
                        "ARA-EXP-APL-008: {} has a stage without Metal artifacts; cook it with Metal Shader Converter",
                        path.display()
                    )
                })?;
                for field in ["metallib", "reflection"] {
                    let name = cooked
                        .get(field)
                        .and_then(serde_json::Value::as_str)
                        .context("cooked Metal stage field")?;
                    ara_pack::validate_path(name)?;
                    let artifact = path.parent().context("pipeline directory")?.join(name);
                    artifact_ready(
                        &artifact,
                        cooked
                            .get(format!("{field}_hash"))
                            .and_then(serde_json::Value::as_str),
                    )?;
                    let relative = artifact
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    files.insert(format!("{prefix}/{relative}"), artifact);
                }
                continue;
            }
            ensure!(
                stage
                    .get("spirv")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "{} has a stage without SPIR-V",
                path.display()
            );
            for field in ["spirv", "dxil"] {
                let Some(name) = stage.get(field).and_then(serde_json::Value::as_str) else {
                    continue;
                };
                ara_pack::validate_path(name)?;
                let artifact = path.parent().context("pipeline directory")?.join(name);
                artifact_ready(
                    &artifact,
                    stage
                        .get(format!("{field}_hash"))
                        .and_then(serde_json::Value::as_str),
                )?;
                let relative = artifact
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(format!("{prefix}/{relative}"), artifact);
            }
        }
    }
    Ok(manifests)
}

fn artifact_ready(path: &Path, expected: Option<&str>) -> Result<()> {
    use sha2::{Digest, Sha256};
    ensure!(path.is_file(), "missing cooked artifact {}", path.display());
    if let Some(expected) = expected {
        let actual = format!("{:x}", Sha256::digest(std::fs::read(path)?));
        ensure!(
            actual == expected,
            "cooked artifact checksum differs: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn clean_runtime_bundle_needs_no_slang_reflection_and_excludes_unused_cohorts() {
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
            for (file, bytes) in [
                (".gitignore", "/*/*.json"),
                (
                    ".ara-pipeline-inputs.json",
                    "offline cache, deliberately not JSON",
                ),
                ("cook-notes.txt", "offline compiler notes"),
                ("cook-settings.json", "{\"offline\":true}"),
            ] {
                std::fs::write(root.join(file), bytes).unwrap();
            }
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
        assert!(
            archive
                .file_names()
                .all(|name| !name.ends_with(".spirv.json") && !name.ends_with(".dxil.json"))
        );
        for file in [
            ".gitignore",
            ".ara-pipeline-inputs.json",
            "cook-notes.txt",
            "cook-settings.json",
        ] {
            assert!(
                archive
                    .by_name(&format!("resources/ara-render/{file}"))
                    .is_err(),
                "offline {file} must stay outside the runtime payload"
            );
        }
        let unpacked = scratch.path().join("game");
        let executable =
            crate::runtime::unpack_template(&first, &unpacked, "ara-player.exe").unwrap();
        assert_eq!(std::fs::read(executable).unwrap(), b"prebuilt player");
    }

    #[test]
    fn a_macos_player_takes_metal_artifacts_metal_only_packages_and_the_game_icon() {
        let scratch = tempfile::tempdir().unwrap();
        let player = scratch.path().join("ara-player");
        std::fs::write(&player, [0xcf, 0xfa, 0xed, 0xfe, 1, 2, 3, 4]).unwrap();
        let key = "a".repeat(64);
        for package in [
            "ara-render",
            "ara-render-fidelity",
            "ara-runtime-render",
            "ara-render2d",
            "ara-landscape",
            "ara-vfx-gpu",
            "ara-texture-gpu",
            "ara-gpu",
            "ara-editor-gpu",
        ] {
            let root = scratch.path().join("crates").join(package).join("cooked");
            std::fs::create_dir_all(root.join(&key)).unwrap();
            for file in ["main.spv", "main.metallib", "main.metal.json"] {
                std::fs::write(root.join(&key).join(file), b"cooked").unwrap();
            }
            let value = serde_json::json!({"stages":[{"spirv":format!("{key}/main.spv"),"metal":{"metallib":format!("{key}/main.metallib"),"reflection":format!("{key}/main.metal.json")}}]});
            std::fs::write(
                root.join("main.json"),
                ara_json::to_canonical_string(&ara_json::sort_keys(&value)),
            )
            .unwrap();
        }
        let icons = scratch.path().join("assets/brand/icons/game");
        std::fs::create_dir_all(&icons).unwrap();
        for pixels in [16u32, 32, 64, 128, 256, 512, 1024] {
            let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            png.extend_from_slice(&pixels.to_be_bytes());
            png.extend_from_slice(&pixels.to_be_bytes());
            png.extend_from_slice(&[8, 6, 0, 0, 0]);
            std::fs::write(icons.join(format!("ara-game-{pixels}.png")), png).unwrap();
        }
        let zip = scratch.path().join("macos.zip");
        build_runtime_template(&player, scratch.path(), &zip).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        for name in [
            format!("resources/ara-editor-gpu/{key}/main.metallib"),
            format!("resources/ara-gpu/{key}/main.metal.json"),
            "AppIcon.icns".into(),
        ] {
            assert!(archive.by_name(&name).is_ok(), "{name} missing");
        }
        assert!(
            archive
                .by_name(&format!("resources/ara-render/{key}/main.spv"))
                .is_err()
        );
        let unpacked = scratch.path().join("game");
        crate::runtime::unpack_template(&zip, &unpacked, "ara-player").unwrap();
        assert!(unpacked.join("AppIcon.icns").is_file());
    }
}
