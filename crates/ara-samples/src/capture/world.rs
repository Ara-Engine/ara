//! Loads a project scene the way Play does: every import sidecar is cooked by the
//! import worker through play-host's asset pipeline, the published generations are
//! pinned, and an `ara_runtime::Runtime` is built over the project's documents.
use anyhow::{Context, Result, anyhow, bail};
use ara_asset::{Digest, FileCache};
use ara_doc::Workspace;
use ara_runtime::{
    Runtime, RuntimeConfig,
    assets::{CachedAssets, Models},
    extract::RenderFrame,
};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

/// A built world plus what presentation needs besides the frame.
pub struct World {
    /// The simulation, stepped to the capture tick.
    pub runtime: Runtime,
    /// The project documents (foliage rules, terrain).
    pub workspace: Workspace,
    /// Decoded models by asset ID, for foliage rule assets.
    pub models: Models,
    /// The pinned cooked generations.
    pub assets: CachedAssets,
}

/// Cooks the project's assets (the import worker at `worker`), builds the runtime for
/// `scene` and steps it `ticks` ticks.
pub fn load(root: &Path, scene: &str, worker: &Path, ticks: u32) -> Result<World> {
    let project = ara_play_host::open_project(root).map_err(|d| anyhow!("{d}"))?;
    let (workspace, diags) = project.load(&[]);
    if let Some((file, d)) = diags
        .iter()
        .find(|(_, d)| d.iter().any(ara_diag::Diagnostic::is_error))
    {
        bail!(
            "{file}: {}",
            d.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let id = workspace
        .find(scene)
        .with_context(|| format!("no document {scene}"))?
        .clone();
    let wanted = workspace
        .index()
        .imports()
        .map(|(id, _)| id.to_owned())
        .collect();
    let started = std::time::Instant::now();
    let mut cooker = ara_play_host::assets::Assets::new(root, worker).map_err(|e| anyhow!(e))?;
    let pins = cooker
        .pins(&wanted, Duration::from_secs(3600))
        .map_err(|e| anyhow!(e))?;
    eprintln!(
        "cooked/pinned {} assets in {:.1} s",
        pins.len(),
        started.elapsed().as_secs_f64()
    );
    let pins: BTreeMap<String, Digest> = pins
        .into_iter()
        .map(|(k, v)| Ok((k, Digest(ara_play::unhex(&v).map_err(|e| anyhow!(e))?))))
        .collect::<Result<_>>()?;
    let cache = cooker.cache.clone();
    let runtime = Runtime::new(
        project.db.clone(),
        workspace.clone(),
        id,
        Box::new(CachedAssets::new(FileCache::at(&cache)?, pins.clone())),
        RuntimeConfig {
            seed: 7,
            ..Default::default()
        },
        Vec::new(),
    )?;
    let mut world = World {
        runtime,
        workspace,
        models: Models::default(),
        assets: CachedAssets::new(FileCache::at(&cache)?, pins),
    };
    world.runtime.step(ticks, &[])?;
    Ok(world)
}

impl World {
    /// The current presentation frame.
    pub fn frame(&mut self) -> RenderFrame {
        let mut frame = RenderFrame::default();
        self.runtime.extract(&mut frame);
        frame
    }

    /// The decoded model of an asset ID (foliage rule assets).
    pub fn model(&mut self, asset: &str) -> Result<Arc<ara_runtime::model::Model>> {
        self.models.load(&self.assets, asset)?;
        self.models
            .get(asset)
            .cloned()
            .with_context(|| format!("model {asset} did not load"))
    }
}
