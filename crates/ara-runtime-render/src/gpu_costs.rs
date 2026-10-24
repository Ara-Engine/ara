//! Timestamp components from one native presentation, before statistical reduction.

/// Native GPU command durations for one frame. Their sum is accounting, not a
/// cross-queue wall-clock measurement. Native terrain/foliage are in the selected
/// view's raster or fidelity passes.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct GpuFrameCosts {
    /// Material/pose/skin uploads and acceleration updates before transport.
    pub scene_update_ms: f64,
    /// Compressed texture promotions and binding-table publication.
    pub texture_upload_ms: f64,
    /// Particle simulation and drawing, excluding their shared post replay.
    pub effects_ms: f64,
    /// Fidelity pass timestamps, including terrain, foliage and post replays.
    pub fidelity_ms: f64,
    /// Native raster G-buffer, shadow, lighting and shared post pass timestamps.
    pub raster_ms: f64,
    /// Copy into the retained completed image; excludes OS/shared-surface display.
    pub publication_ms: f64,
    /// Compressed bytes uploaded by this frame's texture refinement.
    pub texture_uploaded_bytes: u64,
    /// Texture identities still waiting for their budgeted resident mip tail.
    pub texture_pending: usize,
    /// Compressed image payload resident after refinement; excludes upload staging.
    pub texture_resident_bytes: u64,
}

impl GpuFrameCosts {
    /// Sum timestamp components before computing frame-window quantiles.
    pub fn accounted_ms(self) -> f64 {
        self.scene_update_ms
            + self.texture_upload_ms
            + self.effects_ms
            + self.fidelity_ms
            + self.raster_ms
            + self.publication_ms
    }
}
