//! CDLOD reference rules shared with the GPU path and seam tests.
use crate::{TileCoord, invalid};
use ara_diag::Diagnostic;

/// Camera-centred distance bands for a fixed tile grid.
#[derive(Debug, Clone, Copy)]
pub struct LodSettings {
    /// Tile width in metres.
    pub width: f32,
    /// Base distance in metres; each successive band doubles it.
    pub distance: f32,
    /// Coarsest step is 2^max_level, up to the tile cell count.
    pub max_level: u32,
}
impl LodSettings {
    /// Selects detail using distance to the nearest point of the patch bounds.
    pub fn level(
        &self,
        camera: [f32; 2],
        origin: [f32; 2],
        coord: TileCoord,
    ) -> Result<u32, Diagnostic> {
        if !self.width.is_finite()
            || self.width <= 0.
            || !self.distance.is_finite()
            || self.distance <= 0.
            || self.max_level > 8
            || !camera.iter().chain(&origin).all(|v| v.is_finite())
        {
            return Err(invalid("invalid CDLOD parameters"));
        }
        let min = [
            origin[0] + coord.x as f32 * self.width,
            origin[1] + coord.z as f32 * self.width,
        ];
        let delta = [
            camera[0] - camera[0].clamp(min[0], min[0] + self.width),
            camera[1] - camera[1].clamp(min[1], min[1] + self.width),
        ];
        let d = delta[0].hypot(delta[1]);
        Ok(if d <= self.distance {
            0
        } else {
            (d / self.distance).log2().floor() as u32 + 1
        }
        .min(self.max_level))
    }
}

/// Collapses fine border vertices onto the coarser neighbor grid. Both sides then
/// evaluate exactly the same global height samples, without skirts or overlaps.
pub fn stitch_edge(sample: u32, local_level: u32, neighbor_level: u32) -> u32 {
    let step = 1u32 << local_level.max(neighbor_level).min(8);
    sample / step * step
}

/// Fine-to-coarse interior geomorph; shared edges use `stitch_edge` instead.
pub fn morph_sample(sample: f32, level: u32, blend: f32) -> f32 {
    let step = (1u32 << (level + 1).min(8)) as f32;
    sample + ((sample / step).floor() * step - sample) * blend.clamp(0., 1.)
}
