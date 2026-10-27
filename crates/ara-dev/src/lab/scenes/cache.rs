//! Keep scene content independent of another checkout or benchmark's cleanup.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn retain(base: &Path, scene: &str, source: &Path) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    let manifest = source.join("ara-launch.json");
    let launch: Value = serde_json::from_slice(&fs::read(&manifest)?)?;
    let pack = Path::new(launch["pack"].as_str().context("launch pack path")?);
    ensure!(
        pack.is_relative()
            && pack
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "launch pack must stay inside its exported folder"
    );
    ensure!(
        source.join(pack).is_file(),
        "scene content missing: {}",
        source.join(pack).display()
    );
    if source.starts_with(base.canonicalize()?) {
        return Ok(source);
    }
    let folder = base.join("packs");
    fs::create_dir_all(&folder)?;
    let default = folder.join(scene);
    let output = if default.exists() {
        folder.join(format!("{scene}-{}", super::super::session::id()))
    } else {
        default
    };
    let staging = folder.join(format!("pending-{}", super::super::session::id()));
    fs::create_dir_all(&staging)?;
    let destination = staging.join(pack);
    fs::create_dir_all(destination.parent().context("scene pack parent")?)?;
    fs::copy(source.join(pack), destination)?;
    if source.join("game").is_dir() {
        super::super::watch::copy_tree(&source.join("game"), &staging.join("game"))?;
    }
    fs::copy(manifest, staging.join("ara-launch.json"))?;
    super::super::session::write(
        &staging.join("lab-source.json"),
        &serde_json::json!({"source":source,"pack":pack}),
    )?;
    fs::rename(staging, &output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_scene_survives_external_cleanup_and_rejects_path_escape() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::super::session::id());
        let source = root.join("external");
        let base = root.join("lab");
        fs::create_dir_all(source.join("game"))?;
        fs::create_dir_all(&base)?;
        fs::write(
            source.join("ara-launch.json"),
            br#"{"pack":"game.arapack"}"#,
        )?;
        fs::write(source.join("game.arapack"), b"immutable scene")?;
        fs::write(source.join("game/Scripts.dll"), b"gameplay")?;
        fs::write(source.join("ara-player.exe"), b"duplicate player")?;
        let retained = retain(&base, "showcase", &source)?;
        fs::remove_dir_all(&source)?;
        assert_eq!(fs::read(retained.join("game.arapack"))?, b"immutable scene");
        assert!(retained.join("game/Scripts.dll").is_file());
        assert!(!retained.join("ara-player.exe").exists());
        assert_eq!(
            retain(&base, "showcase", &retained)?,
            retained.canonicalize()?
        );
        fs::create_dir_all(&source)?;
        fs::write(source.join("ara-launch.json"), br#"{"pack":"../private"}"#)?;
        assert!(retain(&base, "showcase", &source).is_err());
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
