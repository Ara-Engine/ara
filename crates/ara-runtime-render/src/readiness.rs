//! Renderer-local readiness; external asset owners must also settle publication.
use crate::Presenter;
use serde::Serialize;

/// Cheap, read-only raster readiness after a completed presentation. No I/O or
/// waiting; ordinary motion is allowed. External AssetSource/import work belongs
/// to its owner and must be checked separately for live, unpacked projects.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresentationReadiness {
    /// The selected path is native raster; experimental RT is not covered.
    pub raster: bool,
    /// At least one completed image has been published.
    pub published: bool,
    /// Opaque adopted content key; valid when `content_prepared` is true.
    pub generation: u64,
    /// Native CPU/GPU content has been adopted.
    pub content_prepared: bool,
    /// Assets still represented by unpublished-content placeholders.
    pub unresolved_assets: usize,
    /// The last frame adopted a replacement generation; wait for a stable frame.
    pub content_rebuilt: bool,
    /// Published models are still fading into the image.
    pub model_fades_pending: bool,
    /// An emitter search/rerun has not yet been adopted.
    pub emitter_search_pending: bool,
    /// Remaining residency promotions, or None before native residency exists.
    pub textures_pending: Option<usize>,
    /// Texture bytes promoted in the last frame; zero denotes a settled frame.
    pub texture_uploaded_bytes: u64,
    /// The selected raster preset requests acquired probe GI.
    pub gi_enabled: bool,
    /// Probe acquisition or an incremental edit remains queued/running.
    pub gi_jobs_pending: bool,
    /// Delivered probe records still waiting for renderer adoption.
    pub gi_records_pending: usize,
    /// Acquired probes are scattered, converged and fully visible in shading.
    pub gi_volume_ready: bool,
    /// An asynchronous frame has not completed yet.
    pub gpu_in_flight: bool,
    /// The last presentation failed; `Presenter::presentation_error` names it.
    pub failed: bool,
}

impl PresentationReadiness {
    /// All renderer-local content, residency and lighting have settled. Continue
    /// presenting until this holds, then apply the caller's configured warmup.
    pub fn is_ready(self) -> bool {
        self.raster
            && self.published
            && self.content_prepared
            && !self.failed
            && self.unresolved_assets == 0
            && !self.content_rebuilt
            && !self.model_fades_pending
            && !self.emitter_search_pending
            && self.textures_pending == Some(0)
            && self.texture_uploaded_bytes == 0
            && !self.gpu_in_flight
            && (!self.gi_enabled
                || (!self.gi_jobs_pending && self.gi_records_pending == 0 && self.gi_volume_ready))
    }
}

impl Presenter {
    /// Raster readiness with reasons for a benchmark/loading timeout. Call after
    /// `present`; this never waits and does not treat moving actors as pending.
    pub fn readiness(&self) -> PresentationReadiness {
        let mut status = match (&self.raster, &self.renderer) {
            (Some(raster), Some(renderer)) => raster.readiness(renderer),
            _ => PresentationReadiness::default(),
        };
        status.published = self.published.is_some();
        status.content_rebuilt = self.stats.rebuilt;
        status.failed = self.failure.is_some();
        status
    }
}

#[cfg(test)]
mod tests;
