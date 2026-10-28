//! Preview numbering and a narrowly scoped version commit in an isolated checkout.
use anyhow::{Context, Result, ensure};
use semver::Version;
use std::{fs, path::Path};
use toml_edit::{DocumentMut, Item};

pub(super) fn workspace_version(root: &Path) -> Result<Version> {
    let document = fs::read_to_string(root.join("Cargo.toml"))?.parse::<DocumentMut>()?;
    Version::parse(
        document["workspace"]["package"]["version"]
            .as_str()
            .context("workspace.package.version must be a string")?,
    )
    .map_err(Into::into)
}

/// Numbered alpha previews continue their patch; a shipped unnumbered version
/// starts the next patch's preview stream. Local prepared versions reserve numbers.
pub(super) fn next(
    current: &Version,
    prepared: impl IntoIterator<Item = String>,
) -> Result<Version> {
    let mut base = current.clone();
    let counter = base
        .pre
        .as_str()
        .strip_prefix("alpha.")
        .and_then(|n| n.parse::<u64>().ok());
    if counter.is_none() {
        base.patch = base
            .patch
            .checked_add(1)
            .context("version patch overflow")?;
    }
    base.pre = semver::Prerelease::EMPTY;
    base.build = semver::BuildMetadata::EMPTY;
    let mut number = counter.unwrap_or(0);
    for version in prepared {
        let Ok(version) = Version::parse(&version) else {
            continue;
        };
        if (version.major, version.minor, version.patch) == (base.major, base.minor, base.patch)
            && let Some(n) = version
                .pre
                .as_str()
                .strip_prefix("alpha.")
                .and_then(|n| n.parse::<u64>().ok())
        {
            number = number.max(n);
        }
    }
    let number = number.checked_add(1).context("preview number overflow")?;
    base.pre = semver::Prerelease::new(&format!("alpha.{number}"))?;
    Ok(base)
}

pub(super) fn validate(version: &Version, current: &Version) -> Result<()> {
    ensure!(
        version > current && version.build.is_empty(),
        "preview version must be newer than {current} and have no build suffix"
    );
    ensure!(
        version
            .pre
            .as_str()
            .strip_prefix("alpha.")
            .is_some_and(|n| !n.is_empty() && n.parse::<u64>().is_ok_and(|n| n > 0)),
        "preview version must use alpha.N, N >= 1"
    );
    Ok(())
}

/// External lockfile resolutions must survive a workspace-only version bump.
pub(super) fn external_resolutions(lock: &str) -> Result<std::collections::BTreeSet<String>> {
    let lock = lock.parse::<DocumentMut>()?;
    Ok(lock["package"]
        .as_array_of_tables()
        .context("Cargo.lock packages")?
        .iter()
        .filter(|p| p.get("source").is_some())
        .map(|p| {
            format!(
                "{}|{}|{}|{}",
                p["name"].as_str().unwrap_or(""),
                p["version"].as_str().unwrap_or(""),
                p["source"].as_str().unwrap_or(""),
                p.get("checksum").and_then(Item::as_str).unwrap_or("")
            )
        })
        .collect())
}

/// Local (source-less) lockfile entries still at `old`: a bump that left any behind
/// would make the next cargo build rewrite Cargo.lock and dirty the version commit.
/// Path crates with their own versions (vendored forks) never match `old`.
pub(super) fn stale_local_entries(lock: &str, old: &Version) -> Result<Vec<String>> {
    let lock = lock.parse::<DocumentMut>()?;
    let old = old.to_string();
    Ok(lock["package"]
        .as_array_of_tables()
        .context("Cargo.lock packages")?
        .iter()
        .filter(|p| p.get("source").is_none() && p["version"].as_str() == Some(old.as_str()))
        .map(|p| p["name"].as_str().unwrap_or("").to_owned())
        .collect())
}

/// Sets the workspace version. `RELEASE_NOTES.md` is kept when it already carries
/// reviewed notes for this version (first line `# Ara <version>`); otherwise it is
/// replaced by a generic note naming `source`.
pub(super) fn bump(root: &Path, version: &Version, source: &str) -> Result<()> {
    let path = root.join("Cargo.toml");
    let mut document = fs::read_to_string(&path)?.parse::<DocumentMut>()?;
    document["workspace"]["package"]["version"] = toml_edit::value(version.to_string());
    fs::write(path, document.to_string())?;
    let notes_path = root.join("RELEASE_NOTES.md");
    let heading = format!("# Ara {version}\n");
    if fs::read_to_string(&notes_path).is_ok_and(|notes| notes.starts_with(&heading)) {
        return Ok(());
    }
    let notes = format!(
        "{heading}\nPreview of source commit `{source}`.\n\nThis Preview-channel build includes the changes in that committed source.\nUse Hub's Preview channel to receive it.\n"
    );
    fs::write(notes_path, notes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numbering_reserves_prepared_versions_and_advances_only_the_preview_stream() {
        let first = Version::parse("0.0.1-alpha").unwrap();
        assert_eq!(next(&first, []).unwrap().to_string(), "0.0.2-alpha.1");
        assert_eq!(
            next(&first, ["0.0.2-alpha.4".into(), "bad".into()])
                .unwrap()
                .to_string(),
            "0.0.2-alpha.5"
        );
        assert_eq!(
            next(&Version::parse("0.0.2-alpha.5").unwrap(), [])
                .unwrap()
                .to_string(),
            "0.0.2-alpha.6"
        );
        for version in [
            "0.0.1-alpha.1",
            "0.0.2",
            "0.0.2-beta.1",
            "0.0.2-alpha.0",
            "0.0.2-alpha.1+other",
        ] {
            assert!(
                validate(
                    &Version::parse(version).unwrap(),
                    &Version::parse("0.0.2-alpha").unwrap()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn bump_keeps_reviewed_notes_for_this_version_and_replaces_others() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/tests/bump-notes")
            .join(std::process::id().to_string());
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion = \"0.0.2-alpha.3\"\n",
        )
        .unwrap();
        let version = Version::parse("0.0.2-alpha.4").unwrap();
        let reviewed = "# Ara 0.0.2-alpha.4\n\nReviewed notes.\n";
        fs::write(root.join("RELEASE_NOTES.md"), reviewed).unwrap();
        bump(&root, &version, "abc").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("RELEASE_NOTES.md")).unwrap(),
            reviewed
        );
        fs::write(
            root.join("RELEASE_NOTES.md"),
            "# Ara 0.0.2-alpha.3\n\nOld.\n",
        )
        .unwrap();
        bump(&root, &version, "abc").unwrap();
        let generic = fs::read_to_string(root.join("RELEASE_NOTES.md")).unwrap();
        assert!(generic.starts_with("# Ara 0.0.2-alpha.4\n") && generic.contains("`abc`"));
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn lock_resolution_comparison_ignores_only_workspace_packages() {
        let before = "version = 4\n[[package]]\nname = 'ara'\nversion = '0.0.1-alpha'\n[[package]]\nname = 'serde'\nversion = '1.0.229'\nsource = 'registry+test'\nchecksum = 'original'\n";
        let bumped = before.replace("0.0.1-alpha", "0.0.2-alpha.1");
        assert_eq!(
            external_resolutions(before).unwrap(),
            external_resolutions(&bumped).unwrap()
        );
        assert_ne!(
            external_resolutions(before).unwrap(),
            external_resolutions(&before.replace("original", "changed")).unwrap()
        );
    }
    #[test]
    fn stale_local_entries_name_unbumped_workspace_packages_only() {
        let old = Version::parse("0.0.1-alpha").unwrap();
        let lock = "version = 4\n[[package]]\nname = 'ara'\nversion = '0.0.1-alpha'\n[[package]]\nname = 'lightyear_sync'\nversion = '0.31.0'\n[[package]]\nname = 'serde'\nversion = '0.0.1-alpha'\nsource = 'registry+test'\n";
        assert_eq!(stale_local_entries(lock, &old).unwrap(), ["ara"]);
        let bumped = lock.replacen("0.0.1-alpha", "0.0.2-alpha.1", 1);
        assert!(stale_local_entries(&bumped, &old).unwrap().is_empty());
    }
}
