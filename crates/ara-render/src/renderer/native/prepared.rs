//! An owned validation proof and worker-packed geometry for one publication.
use super::*;
use crate::instanced::{CheckedScene, GeometryPreparation, GeometrySeed, geometry_storage_limit};

/// Read-only reuse metadata captured from the live renderer at a content boundary.
/// The previous geometry must remain published until the candidate is adopted.
pub struct NativeSeed {
    previous: Option<GeometrySeed>,
    limit: u64,
    shadow_slots: usize,
    /// The device has ray queries: geometry is allocated as acceleration-structure
    /// input for the hybrid High/Epic path (`renderer::rt`).
    ray_inputs: bool,
}

/// Immutable CPU-prepared native content. Construct on the content worker, poll
/// `preallocate`, then publish or explicitly `destroy` on its creating renderer.
#[must_use = "publish native content or destroy its preparation"]
pub struct NativeContent {
    scene: CheckedScene,
    textures: Box<dyn Textures>,
    geometry: GeometryPreparation,
    shadow_slots: usize,
}

impl NativeContent {
    /// Validate and pack without device access. A seed retains static GPU reuse
    /// across content generations; no seed prepares a fully new pool.
    pub fn new(
        scene: NativeScene,
        textures: Box<dyn Textures>,
        seed: Option<NativeSeed>,
    ) -> Result<Self> {
        ara_gpu::cpu_scope!("RasterNative.prepare_cpu");
        let scene = CheckedScene::new(scene)?;
        let geometry = GeometryPreparation::new(
            scene.validated(),
            seed.as_ref().map_or(u64::from(u32::MAX), |seed| seed.limit),
            seed.as_ref().and_then(|seed| seed.previous.as_ref()),
            seed.as_ref().is_some_and(|seed| seed.ray_inputs),
        )?;
        let shadow_slots = seed.as_ref().map_or(0, |seed| seed.shadow_slots);
        Ok(Self {
            scene,
            textures,
            geometry,
            shadow_slots,
        })
    }

    /// The immutable snapshot that will be published with the packed geometry.
    pub fn scene(&self) -> &NativeScene {
        self.scene.validated().scene()
    }

    /// Reserve all face/cascade patch rows for the publication's quality. This
    /// changes CPU metadata only; publication keeps the larger source capacity.
    pub fn reserve_shadow_quality(&mut self, quality: &crate::look::Quality) -> Result<()> {
        quality.validate()?;
        self.shadow_slots = self
            .shadow_slots
            .max(Renderer::shadow_slots_for_quality(quality));
        Ok(())
    }

    /// Poll allocation/staging only, with both jobs bounded by the device worker.
    /// Call at a completed boundary; no queue work or host waits occur here.
    pub fn preallocate(&mut self, renderer: &mut Renderer) -> Result<bool> {
        ensure!(
            self.geometry
                .source_matches(renderer.native.as_ref().map(|n| &n.pool)),
            "native prepared geometry source changed; prepare a new candidate"
        );
        let textures = renderer.preallocate_native_textures(self.textures.as_mut())?;
        let geometry = self.geometry.preallocate(&mut renderer.gpu)?;
        let pipelines = renderer.preallocate_native_pipelines()?;
        Ok(textures && geometry && pipelines)
    }

    /// Cancel work and retire adopted allocations without altering last-good.
    pub fn destroy(self, renderer: &mut Renderer) -> Result<()> {
        self.geometry.destroy(&mut renderer.gpu)?;
        self.textures.destroy(&mut renderer.gpu)?;
        renderer.gpu.collect()?;
        Ok(())
    }
}

impl Renderer {
    /// Capture small immutable packing metadata for the next content worker.
    pub fn native_seed(&self) -> NativeSeed {
        self.native_seed_inner(self.shadow_slots())
    }

    /// Capture reuse and shadow-row capacity for a requested quality before
    /// starting its content worker, without changing the currently shown view.
    pub fn native_seed_for_quality(&self, quality: &crate::look::Quality) -> Result<NativeSeed> {
        quality.validate()?;
        Ok(self.native_seed_inner(Self::shadow_slots_for_quality(quality)))
    }

    fn native_seed_inner(&self, shadow_slots: usize) -> NativeSeed {
        NativeSeed {
            previous: self.native.as_ref().map(|n| n.pool.preparation_seed()),
            limit: geometry_storage_limit(&self.gpu),
            ray_inputs: self.gpu.info().ray_tracing,
            shadow_slots: shadow_slots.max(
                self.native
                    .as_ref()
                    .map_or(0, |n| n.capacity.saturating_sub(culling::BASE_PASSES)),
            ),
        }
    }

    /// Publish a worker-prepared generation through the normal ordered upload
    /// scope. Returns the CPU scene for later in-place pose/material updates.
    /// Failure destroys candidate resources and keeps the current scene.
    pub fn publish_native_content(&mut self, content: NativeContent) -> Result<NativeScene> {
        if !content
            .geometry
            .source_matches(self.native.as_ref().map(|n| &n.pool))
        {
            content.destroy(self)?;
            anyhow::bail!("native prepared geometry source changed before publication");
        }
        let NativeContent {
            scene,
            textures,
            geometry,
            shadow_slots,
        } = content;
        let pool = match geometry.publish(&mut self.gpu) {
            Ok(pool) => pool,
            Err(error) => {
                textures.destroy(&mut self.gpu)?;
                return Err(error);
            }
        };
        self.publish_native_pool(
            scene.validated(),
            textures,
            Some(pool),
            shadow_slots.max(self.shadow_slots()),
        )?;
        Ok(scene.into_scene())
    }
}
