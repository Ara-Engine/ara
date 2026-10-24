//! Loads a project scene the way Play does: every import sidecar is cooked by the
//! import worker through play-host's asset pipeline, the published generations are
//! pinned, and an `ara_runtime::Runtime` is built over the project's documents.
use anyhow::{Context, Result, anyhow, bail};
use ara_asset::{Digest, FileCache};
use ara_runtime::{Runtime, RuntimeConfig, assets::CachedAssets, extract::RenderFrame};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// A built world plus what presentation needs besides the frame.
pub struct World {
    /// The simulation, stepped to the capture tick.
    pub runtime: Runtime,
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
        workspace,
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
        assets: CachedAssets::new(FileCache::at(&cache)?, pins),
    };
    world.runtime.step(ticks, &[])?;
    complete_assets(&world.runtime)?;
    Ok(world)
}

/// Reference scores are meaningless when extraction substituted missing assets.
fn complete_assets(runtime: &Runtime) -> Result<()> {
    anyhow::ensure!(
        runtime.asset_errors().is_empty(),
        "capture requires complete scene assets; fix these runtime diagnostics before scoring:\n{}",
        runtime.asset_errors().join("\n")
    );
    Ok(())
}

impl World {
    /// Resolves the shipping adapter's terrain materials and prefab foliage from
    /// this runtime's immutable cooked generations and material/model library.
    pub fn content(
        &self,
        frame: &RenderFrame,
    ) -> Result<ara_runtime_render::fidelity::native::Content> {
        ara_runtime_render::fidelity::native::Content::resolve(
            frame,
            self.runtime.documents(),
            self.runtime.schema(),
            self.runtime
                .world()
                .get_resource::<ara_runtime::animation::Library>()
                .context("runtime material/model library")?,
            &self.assets,
        )
    }
    /// The current presentation frame.
    pub fn frame(&mut self) -> RenderFrame {
        let mut frame = RenderFrame::default();
        self.runtime.extract(&mut frame);
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_placeholder_cannot_be_used_as_capture_ground_truth() -> Result<()> {
        let schema = ara_runtime::schema(&[])?;
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-player/demo/levels/demo.scene");
        let read = ara_doc::read(
            &std::fs::read_to_string(path)?,
            "levels/demo.scene",
            &schema,
            None,
        );
        let document = read.doc.context("demo scene")?;
        let scene = document.id.clone();
        let mut workspace = ara_doc::Workspace::default();
        workspace.insert("levels/demo.scene", document);
        let runtime = Runtime::new(
            schema,
            workspace,
            scene,
            Box::new(ara_runtime::assets::MemoryAssets::default()),
            RuntimeConfig::default(),
            Vec::new(),
        )?;
        let error = complete_assets(&runtime).expect_err("missing fox must invalidate reference");
        assert!(error.to_string().contains("ARA7310"));
        assert!(error.to_string().contains("as_fox"));
        Ok(())
    }
}
