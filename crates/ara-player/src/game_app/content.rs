//! The same authored snapshot for source Play and immutable Windows exports.
use anyhow::{Context, Result, ensure};
use ara_play::{GameUi, PlayScene};
use ara_runtime::assets::AssetSource;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct Content {
    pub scene: PlayScene,
    pub source: Option<Arc<dyn AssetSource>>,
}

pub(super) fn ui(project: &Path) -> Result<GameUi> {
    ara_play_host::read_game_ui(project)
        .map_err(anyhow::Error::msg)?
        .context("this project has no native game UI")
}

pub(super) fn project(
    root: &Path,
    report: impl FnMut(ara_play_host::assets::CookProgress) -> Result<(), String>,
) -> Result<Content> {
    let worker = std::env::var_os("ARA_IMPORT_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap_or_default()
                .with_file_name(format!("ara-import-worker{}", std::env::consts::EXE_SUFFIX))
        });
    Ok(Content {
        scene: ara_play_host::package_scene_with_progress(root, &worker, report)
            .map_err(anyhow::Error::msg)?,
        source: None,
    })
}

pub(super) fn exported(root: &Path) -> Result<Content> {
    use ara_pack::{EncryptionKey, OwnedPack};
    let launch: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("ara-launch.json"))?)?;
    ensure!(
        launch["contract"] == "ara.player.runtime/1",
        "unsupported exported player contract"
    );
    let name = launch["pack"].as_str().context("pack path")?;
    ara_pack::validate_path(name)?;
    let key = launch["encryption_key_env"]
        .as_str()
        .map(|env| -> Result<_> {
            Ok(EncryptionKey::from_hex(
                &std::env::var(env).context("pack encryption key unset")?,
            )?)
        })
        .transpose()?;
    let pack = Arc::new(OwnedPack::open(std::fs::read(root.join(name))?)?);
    let mut scene = PlayScene::decode(&pack.view().read(
        ".ara/scene.cbor",
        64 * 1024 * 1024,
        key.as_ref(),
    )?)
    .map_err(anyhow::Error::msg)?;
    scene.nav_cache = None;
    if let Some(scripts) = &mut scene.scripts {
        ensure!(
            matches!(scripts.host, ara_play::ScriptHost::NativeAot { .. }),
            "exported gameplay requires a NativeAOT host"
        );
        scripts.resolve_shipping(root);
    }
    let source = Arc::new(crate::packed::PackedAssets::new(pack, key, &scene)?);
    Ok(Content {
        scene,
        source: Some(source),
    })
}
