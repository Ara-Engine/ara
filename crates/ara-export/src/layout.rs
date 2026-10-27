//! The player-facing folder contains one application, one data folder and notices.
use crate::{ExportPreset, Platform, tools};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// Resolves an application name independently of the gameplay assembly name.
pub(crate) fn game_name(project: &Path, preset: &ExportPreset) -> Result<String> {
    let project_name = std::fs::read(project.join("ara.project.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("name")?.as_str().map(str::to_owned));
    let name = preset
        .name
        .as_ref()
        .or(project_name.as_ref())
        .filter(|name| !name.trim().is_empty())
        .map_or(preset.assembly.as_str(), String::as_str);
    validate_name(name)?;
    Ok(name.to_owned())
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.bytes().all(|b| (b'1'..=b'9').contains(&b)))
        });
    ensure!(
        !name.is_empty()
            && name.trim() == name
            && name.len() <= 120
            && !name.ends_with('.')
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            && !matches!(name, "." | "..")
            && !reserved,
        "ARA-EXP-LAYOUT-001: game name must be a portable filename, without reserved names or path characters"
    );
    Ok(())
}

pub(crate) fn version_parts(version: &str) -> Result<[u16; 4]> {
    let parts: Vec<_> = version.split('.').collect();
    ensure!(
        (1..=4).contains(&parts.len()),
        "ARA-EXP-LAYOUT-001: game version needs one to four numeric components"
    );
    let mut result = [0; 4];
    for (at, part) in parts.into_iter().enumerate() {
        ensure!(
            !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()),
            "ARA-EXP-LAYOUT-001: game version needs numeric components"
        );
        result[at] = part
            .parse()
            .context("ARA-EXP-LAYOUT-001: game version component exceeds 65535")?;
    }
    Ok(result)
}

pub(crate) fn executable(name: &str, platform: Platform) -> String {
    if platform == Platform::Windows {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Moves the assembled legacy payload into its private data folder, preserving
/// relative launch/script/resource references and keeping notices discoverable.
pub(crate) fn desktop(build: &Path, name: &str, platform: Platform) -> Result<PathBuf> {
    let source = build.join(if platform == Platform::Windows {
        "ara-player.exe"
    } else {
        "ara-player"
    });
    ensure!(
        source.is_file(),
        "ARA-EXP-LAYOUT-001: assembled player is missing"
    );
    let player = build.join(executable(name, platform));
    let data = build.join("data");
    std::fs::create_dir(&data)?;
    for entry in std::fs::read_dir(build)? {
        let entry = entry?;
        let path = entry.path();
        if path == data || path == source || notices(&entry.file_name().to_string_lossy()) {
            continue;
        }
        std::fs::rename(&path, data.join(entry.file_name()))?;
    }
    if source != player {
        std::fs::rename(source, &player)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&player, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(player)
}

pub(crate) fn notices(name: &str) -> bool {
    matches!(
        name,
        "THIRD_PARTY_NOTICES.txt" | "NOTICES.txt" | "LICENSE.txt"
    )
}

pub(crate) fn write_notices(project: &Path, staged: &Path) -> Result<()> {
    if !staged.join("THIRD_PARTY_NOTICES.txt").is_file() {
        std::fs::write(
            staged.join("THIRD_PARTY_NOTICES.txt"),
            include_bytes!("../../../THIRD_PARTY_NOTICES.md"),
        )?;
    }
    for name in ["NOTICES.txt", "LICENSE.txt"] {
        if project.join(name).is_file() {
            tools::copy(&project.join(name), &staged.join(name))?;
        }
    }
    Ok(())
}

/// Rejects accidentally staged diagnostic evidence before any shipping package.
pub(crate) fn audit(build: &Path) -> Result<()> {
    for name in crate::distribution::files(build)?.keys() {
        let normalized = name.to_ascii_lowercase();
        let path = Path::new(&normalized);
        let private = path
            .file_name()
            .is_some_and(|file| file.to_string_lossy().starts_with("verify."))
            || path.extension().is_some_and(|extension| {
                matches!(extension.to_str(), Some("pdb" | "dbg" | "dmp" | "log"))
            })
            || path.components().any(|part| {
                matches!(
                    part.as_os_str().to_str(),
                    Some("logs" | "symbols" | "report")
                )
            });
        ensure!(
            !private,
            "ARA-EXP-LAYOUT-001: private export artifact {name} cannot be shipped"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distribution_relocates_as_one_application_and_one_data_folder() {
        let root = tempfile::tempdir().expect("scratch directory");
        let build = root.path().join("build");
        std::fs::create_dir_all(build.join("game")).expect("payload");
        for name in [
            "ara-player.exe",
            "ara-launch.json",
            "game.arapack",
            "engine.json",
            "ara-crash.json",
            "game/Game.dll",
            "THIRD_PARTY_NOTICES.txt",
        ] {
            std::fs::write(build.join(name), name).expect("fixture");
        }
        let player = desktop(&build, "Outpost", Platform::Windows).expect("layout");
        assert_eq!(player, build.join("Outpost.exe"));
        let mut names: Vec<_> = std::fs::read_dir(&build)
            .expect("build")
            .map(|e| e.expect("entry").file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["Outpost.exe", "THIRD_PARTY_NOTICES.txt", "data"].map(std::ffi::OsString::from)
        );
        let moved = root.path().join("relocated");
        std::fs::rename(build, &moved).expect("relocate");
        assert!(moved.join("data/game/Game.dll").is_file());
        assert!(moved.join("data/ara-launch.json").is_file());
        audit(&moved).expect("shipping payload");
        std::fs::write(moved.join("data/verify.render_ids.bin"), b"private").expect("fixture");
        assert!(audit(&moved).is_err());
    }
    #[test]
    fn game_identity_is_portable_and_numeric_versions_are_bounded() {
        for name in ["../Game", "con", "COM1", "Game.", " Game", "Game: Preview"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name("Outpost Preview").is_ok());
        assert_eq!(version_parts("2.4.8").expect("version"), [2, 4, 8, 0]);
        for version in ["1.2.3.4.5", "1.a", "1..2", "65536"] {
            assert!(version_parts(version).is_err());
        }
    }
}
