//! Check the exact installed runtime cohort before packaging or compiling gameplay.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Read-only result; every named manifest/artifact was read and checked.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCohort {
    /// Player binary to package.
    pub player: PathBuf,
    /// Engine installation or explicit source checkout supplying cooked resources.
    pub engine: PathBuf,
    /// Windows companion whose GUID/age matches the player, if applicable.
    pub symbols: Option<PathBuf>,
    /// Number of shader manifests, binaries and reflection files in the cohort.
    pub resource_files: usize,
}

/// Verify a complete player/resource cohort without writing files or starting tools.
/// On Windows `symbols` is required and must match the PE's PDB GUID/age.
pub fn runtime(player: &Path, engine: &Path, symbols: Option<&Path>) -> Result<RuntimeCohort> {
    let checked: Result<_> = (|| {
        ensure!(
            player.is_file(),
            "the companion player is missing: {}",
            player.display()
        );
        let inventory = crate::runtime_template::inventory(player, engine)?;
        if player
            .extension()
            .is_some_and(|extension| extension == "exe")
        {
            let symbols = symbols.context("the matching companion player PDB is missing")?;
            windows_symbols(player, symbols)?;
        }
        let resource_files = inventory
            .files
            .keys()
            .filter(|path| path.starts_with("resources/"))
            .count();
        Ok(RuntimeCohort {
            player: player.to_owned(),
            engine: engine.to_owned(),
            symbols: symbols.map(Path::to_owned),
            resource_files,
        })
    })();
    checked.context("ARA-EXP-PREFLIGHT-001: this engine cannot export a complete playable build; repair its runtime cohort")
}

/// Compare a Windows player and PDB without converting symbols or compiling code.
pub fn windows_symbols(player: &Path, symbols: &Path) -> Result<()> {
    let (_, expected) = ara_crash::symbols::pe_debug_identity(player)?;
    let mut pdb = pdb::PDB::open(
        std::fs::File::open(symbols)
            .with_context(|| format!("missing player symbols {}", symbols.display()))?,
    )?;
    let information = pdb.pdb_information()?;
    let guid = information.guid.simple().to_string().to_ascii_uppercase();
    let age = pdb.debug_information()?.age().unwrap_or(information.age);
    let actual = format!("{guid}{age:X}");
    ensure!(
        actual == expected,
        "ARA-EXP-CRASH-005: player/PDB GUID or age differs; repair the exact installed engine build"
    );
    Ok(())
}

/// A catalogued failure with the real pinned-version repair action when installed.
/// Repair never closes projects or silently replaces an engine that is in use.
pub fn diagnostic(engine: &Path, error: &str) -> ara_diag::Diagnostic {
    let marker = std::fs::read(engine.join("engine.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let mut diagnostic = ara_diag::Diagnostic::error(ara_diag::Code(8126), error)
        .subject(ara_diag::Subject::system("export"))
        .data(serde_json::json!({"engine":engine,"requiresClosedProject":true}));
    if let Some(version) = marker
        .as_ref()
        .filter(|value| value["format"] == "ara.engine/1")
        .and_then(|value| value["version"].as_str())
    {
        diagnostic.fixes.push(ara_diag::Fix {
            title: format!("Close this project, then repair Ara {version}"),
            action: ara_diag::FixAction::Cli {
                args: vec!["hub".into(), "repair".into(), version.into()],
            },
        });
    } else {
        diagnostic.message.push_str(". In a checkout, rebuild the matching player/PDB through ara-dev and re-cook the missing shader manifests before retrying.");
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_installed_cohort_reports_the_exact_repair_action_without_writes() -> Result<()> {
        let root = tempfile::tempdir()?;
        crate::tools::write_json(
            &root.path().join("engine.json"),
            &serde_json::json!({"format":"ara.engine/1","version":"0.0.1-preview.4"}),
        )?;
        let before = std::fs::read_dir(root.path())?.count();
        let error = runtime(&root.path().join("ara-player.exe"), root.path(), None)
            .expect_err("missing companion");
        let diagnostic = diagnostic(root.path(), &format!("{error:#}"));
        assert_eq!(diagnostic.code, ara_diag::Code(8126));
        assert_eq!(
            diagnostic.fixes[0].action,
            ara_diag::FixAction::Cli {
                args: vec!["hub".into(), "repair".into(), "0.0.1-preview.4".into()]
            }
        );
        assert_eq!(before, std::fs::read_dir(root.path())?.count());
        Ok(())
    }

    #[test]
    fn missing_corrupt_or_empty_cooked_cohorts_fail_before_packaging() -> Result<()> {
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir()?;
        let player = root.path().join("ara-player");
        std::fs::write(&player, b"\x7fELF fixture")?;
        let key = "a".repeat(64);
        for package in [
            "ara-render",
            "ara-render-fidelity",
            "ara-runtime-render",
            "ara-render2d",
            "ara-landscape",
            "ara-vfx-gpu",
        ] {
            let cooked = root.path().join("resources").join(package);
            std::fs::create_dir_all(cooked.join(&key))?;
            std::fs::write(cooked.join(&key).join("main.spv"), b"shader")?;
            std::fs::write(cooked.join(&key).join("main.spirv.json"), b"{}")?;
            crate::tools::write_json(
                &cooked.join("main.json"),
                &serde_json::json!({"stages":[{
                    "spirv":format!("{key}/main.spv"), "spirv_hash":format!("{:x}", Sha256::digest(b"shader"))
                }]}),
            )?;
        }
        assert!(runtime(&player, root.path(), None)?.resource_files > 0);
        let artifact = root
            .path()
            .join("resources/ara-render")
            .join(&key)
            .join("main.spv");
        std::fs::write(&artifact, b"corrupt shader")?;
        let error = runtime(&player, root.path(), None).expect_err("corrupt cohort");
        assert!(format!("{error:#}").contains("checksum differs"));
        std::fs::write(&artifact, b"shader")?;
        std::fs::remove_file(artifact.with_extension("spirv.json"))?;
        assert!(
            runtime(&player, root.path(), None).is_err(),
            "missing reflection"
        );
        assert!(!root.path().join("output.zip").exists());
        Ok(())
    }
}
