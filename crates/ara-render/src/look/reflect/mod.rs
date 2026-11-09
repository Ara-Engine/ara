//! Opt-in render-v3 secondary transport. No screen colour, random sampling,
//! spatial blur or lighting history. Native integration lives in `renderer.rs`.
mod scene;
#[cfg(test)]
mod tests;
pub(crate) use scene::Scene;

/// Explicit experimental software reflections; absence keeps today's renderer.
/// Cuts are experiment inputs, not a quality certification or a preset default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftwareReflections {
    /// At or below this roughness, trace one mirror direction per native pixel.
    pub mirror_cut: f32,
    /// Above this roughness, use today's prefiltered probe/capture lobe.
    pub rough_cut: f32,
    /// Fixed GGX quadrature size in the band between the cuts (1..=64).
    pub rough_rays: u32,
    /// Longest secondary segment, in metres.
    pub distance: f32,
    /// Maximum ordered interfaces on a branch (1..=16). Exhaustion is reported.
    pub interfaces: u32,
}

impl SoftwareReflections {
    pub(crate) fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mirror_cut.is_finite()
                && self.rough_cut.is_finite()
                && (0.0..=1.0).contains(&self.mirror_cut)
                && (self.mirror_cut..=1.0).contains(&self.rough_cut)
                && (1..=64).contains(&self.rough_rays)
                && self.distance.is_finite()
                && self.distance > 0.0
                && (1..=16).contains(&self.interfaces),
            "invalid software reflection experiment settings"
        );
        Ok(())
    }
}

/// Native-pixel counters, read only after a completed render when requested.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ReflectionCounters {
    /// Pixels below the mirror cut.
    pub mirror_pixels: u32,
    /// Pixels in the deterministic rough band, budgeted separately.
    pub rough_pixels: u32,
    /// Reflection rays, including glass branches.
    pub rays: u32,
    /// Secondary visibility rays (any-hit queries).
    pub visibility_rays: u32,
    /// Ordered glass interfaces visited.
    pub glass_interfaces: u32,
    /// Branches that exhausted the interface budget; require a tail-error bound
    /// before promotion rather than silently calling the truncated path converged.
    pub exhausted: u32,
    /// Invalid or overflowing medium nesting; nonzero cannot pass G6.
    pub invalid_media: u32,
    /// Pixels using the prefiltered probe/capture lobe.
    pub prefiltered_pixels: u32,
}
