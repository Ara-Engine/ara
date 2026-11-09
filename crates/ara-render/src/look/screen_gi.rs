//! Screen probe GI (`Quality::screen_gi`; shaders/look-screen-gi.slang): the Hi-Z
//! pyramid of the frame's depth, one probe per screen tile tracing a fixed set of
//! hemisphere directions through it (hits shaded from last frame's colour pyramid,
//! misses from the probe volume's radiance), and a half-resolution gather lighting
//! upsamples. Lumen's screen probes are the pattern; everything here is deterministic
//! (fixed directions and placement, no history), so it is stable without accumulation.
//!
//! Frame order: after the reflection pyramid (which hits read) and the probe relight
//! (which misses read), before any pass reading what lighting reads.
use super::gpu::{
    HIZ, LookGpu, LookIds, SCREEN_DIFFUSE, SCREEN_EMISSION, SCREEN_PROBE_RECORDS, SCREEN_PROBES,
    SCREEN_SPECULAR, SKY_VIEW, SSR_PYRAMID, TRANSMITTANCE,
};
use super::passes::LookPass;
use crate::graph::{Graph, PassId, ResourceId};
use ara_gpu::{Access, Queue};

/// Atlas texels per probe (look-screen-gi.slangh SCREEN_PROBE_TILE): 8x8 traced radiance,
/// four 4x4 GGX-prefiltered levels and the 4x4 irradiance.
pub(crate) const PROBE_TILE: [u32; 2] = [20, 8];
/// Highest Hi-Z level (cells of 2^k render pixels); look-screen.slangh HIZ_MAX_LEVELS - 1.
const MAX_HIZ_LEVEL: u32 = 7;
/// Probe origins sit this many render pixels (at their depth) off the surface.
const PROBE_BIAS_PIXELS: f32 = 2.;

/// Probes per row and column for a render size and tile.
pub(crate) fn grid(render: [u32; 2], tile: u32) -> [u32; 2] {
    render.map(|v| v.div_ceil(tile.max(1)))
}

/// Hi-Z levels for a render size: up to cells of 128 pixels, never past one cell.
pub(crate) fn hiz_levels(render: [u32; 2]) -> u32 {
    let largest = render[0].max(render[1]).max(1);
    MAX_HIZ_LEVEL.min(31 - largest.leading_zeros())
}

/// The Hi-Z atlas: levels 1.. side by side, each ceil(size / 2^k), plus slack for the
/// rounding (one texel per level).
pub(crate) fn hiz_extent(render: [u32; 2]) -> [u32; 2] {
    [render[0] + MAX_HIZ_LEVEL + 1, render[1].div_ceil(2).max(1)]
}

impl LookGpu {
    /// Whether this frame runs screen probe GI: the preset budgets it, its lighting
    /// variant (`high_tier`) reads it, and last frame's colour exists to shade hits.
    pub(crate) fn screen_gi_active(&self, history: bool) -> bool {
        self.quality.screen_gi.is_some() && self.quality.high_tier && history
    }

    /// Sets this frame's screen-trace state: screen probe GI (`frame_screen`) and the
    /// Hi-Z pyramid that it and screen-space reflections traverse (`frame_hiz`).
    pub(crate) fn begin_screen_traces(&mut self, history: bool) {
        self.frame_screen = self.screen_gi_active(history);
        self.frame_hiz = self.frame_screen || (history && self.quality.reflections.is_some());
    }

    /// Declares the Hi-Z levels of the frame's depth (closest depth per cell; SSR and
    /// screen probes traverse it). `reads` must cover the depth and the frame.
    pub(crate) fn hiz_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        const NAMES: [&str; MAX_HIZ_LEVEL as usize] = [
            "HiZ.1", "HiZ.2", "HiZ.3", "HiZ.4", "HiZ.5", "HiZ.6", "HiZ.7",
        ];
        (1..=hiz_levels(self.render))
            .map(|level| {
                let id = graph.pass(
                    NAMES[level as usize - 1],
                    Queue::Graphics,
                    [
                        (ids.image(HIZ), Access::Storage),
                        (ids.frame, Access::ShaderRead),
                    ]
                    .into_iter()
                    .chain(reads.iter().copied()),
                );
                (id, LookPass::Hiz(level))
            })
            .collect()
    }

    /// Declares the emission pyramid, the probe trace and the half-resolution gather
    /// (after `hiz_passes`).
    /// `reads` must cover the G-buffer (depth, normals, materials), the frame and the
    /// probe volume the misses read (its relight declared earlier).
    pub(crate) fn screen_gi_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        let mut passes = Vec::new();
        let hiz = ids.image(HIZ);
        const EMISSION: [&str; 6] = [
            "ScreenGI.Emission1",
            "ScreenGI.Emission2",
            "ScreenGI.Emission3",
            "ScreenGI.Emission4",
            "ScreenGI.Emission5",
            "ScreenGI.Emission6",
        ];
        for (i, &name) in EMISSION.iter().enumerate() {
            let id = graph.pass(
                name,
                Queue::Graphics,
                [
                    (ids.image(SCREEN_EMISSION), Access::Storage),
                    (ids.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(reads.iter().copied()),
            );
            passes.push((id, LookPass::EmissionPyramid(i as u32 + 1)));
        }
        let gi = ids
            .gi
            .map(|gi| [1, 2, 4, 5].map(|i| (gi[i], Access::ShaderRead)));
        let trace = graph.pass(
            "ScreenGI.Probes",
            Queue::Graphics,
            [
                (hiz, Access::ShaderRead),
                (ids.image(SSR_PYRAMID), Access::ShaderRead),
                (ids.image(SCREEN_EMISSION), Access::ShaderRead),
                (ids.image(SCREEN_PROBES), Access::Storage),
                (ids.image(SCREEN_PROBE_RECORDS), Access::Storage),
                (ids.state, Access::ShaderRead),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(gi.into_iter().flatten())
            // Misses outside the probe volume see the physical sky.
            .chain(
                [TRANSMITTANCE, SKY_VIEW]
                    .into_iter()
                    .filter(|_| self.look.sky.is_some())
                    .map(|i| (ids.image(i), Access::ShaderRead)),
            )
            .chain(reads.iter().copied()),
        );
        passes.push((trace, LookPass::ScreenProbes));
        let gather = graph.pass(
            "ScreenGI.Gather",
            Queue::Graphics,
            [
                (ids.image(SCREEN_PROBES), Access::ShaderRead),
                (ids.image(SCREEN_PROBE_RECORDS), Access::ShaderRead),
                (ids.image(SCREEN_DIFFUSE), Access::Storage),
                (ids.image(SCREEN_SPECULAR), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(reads.iter().copied()),
        );
        passes.push((gather, LookPass::ScreenGather));
        passes
    }

    /// The images lighting reads from a frame with screen probe GI.
    pub(crate) fn screen_gi_reads(&self, ids: &LookIds) -> [(ResourceId, Access); 2] {
        [SCREEN_DIFFUSE, SCREEN_SPECULAR].map(|i| (ids.image(i), Access::ShaderRead))
    }

    /// Writes the screen rows (look-screen.slangh): the Hi-Z pyramid's on frames that
    /// build it (`frame_hiz`), the probes' and the `SCREEN_GI` flag on frames with
    /// screen probe GI.
    pub(crate) fn screen_gi_rows(&self, frame: &mut super::frame::LookFrame) {
        // Emission the host promoted to emitter lights lights the scene directly:
        // screen probe hits and reflection capture faces drop it (counted once).
        frame.screen_params[1] = [
            if self.quality.emitter_lights > 0 {
                super::EMITTER_RADIANCE
            } else {
                f32::MAX
            },
            0.,
            0.,
            0.,
        ];
        if self.frame_hiz {
            frame.screen_params[0][1] = hiz_levels(self.render) as f32;
            frame.screen_info[0][0] = self.sampled[HIZ];
            frame.screen_info[0][1] = self.storage[HIZ];
        }
        let Some(budget) = self.quality.screen_gi.filter(|_| self.frame_screen) else {
            return;
        };
        frame.flags[1] |= super::frame::SCREEN_GI;
        let probes = grid(self.render, budget.tile);
        frame.screen_params[0] = [
            budget.steps as f32,
            hiz_levels(self.render) as f32,
            budget.distance,
            PROBE_BIAS_PIXELS,
        ];
        frame.screen_info = [
            [
                self.sampled[HIZ],
                self.storage[HIZ],
                self.sampled[SCREEN_PROBES],
                self.storage[SCREEN_PROBES],
            ],
            [
                self.sampled[SCREEN_PROBE_RECORDS],
                self.storage[SCREEN_PROBE_RECORDS],
                probes[0],
                budget.tile,
            ],
            [
                self.sampled[SCREEN_DIFFUSE],
                self.sampled[SCREEN_SPECULAR],
                self.storage[SCREEN_DIFFUSE],
                self.storage[SCREEN_SPECULAR],
            ],
            [
                self.sampled[SCREEN_EMISSION],
                self.storage[SCREEN_EMISSION],
                0,
                0,
            ],
        ];
    }

    /// Drops the probe-sized images when a new budget changes the tile (`ensure`
    /// reallocates them at the new grid).
    pub(crate) fn retile_screen_probes(
        &mut self,
        gpu: &mut ara_gpu::GpuDevice,
        previous: Option<super::ScreenGi>,
    ) -> anyhow::Result<()> {
        let tile = |s: Option<super::ScreenGi>| s.map(|s| s.tile);
        if tile(previous) == tile(self.quality.screen_gi) {
            return Ok(());
        }
        for i in [SCREEN_PROBES, SCREEN_PROBE_RECORDS] {
            if let Some(old) = self.images[i].take() {
                ara_gpu::Device::retire(gpu, ara_gpu::Resource::Image(old))?;
            }
        }
        Ok(())
    }

    /// The probe trace's dispatch: one group per probe.
    pub(crate) fn screen_probe_groups(&self) -> [u32; 3] {
        let tile = self.quality.screen_gi.map_or(16, |s| s.tile);
        let [x, y] = grid(self.render, tile);
        [x, y, 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiz_levels_stop_at_one_cell() {
        assert_eq!(hiz_levels([1707, 960]), 7);
        assert_eq!(hiz_levels([40, 20]), 5);
        assert_eq!(hiz_levels([1, 1]), 0);
        // Every level's ceil width fits the atlas.
        let render = [1707, 961];
        let width: u32 = (1..=hiz_levels(render))
            .map(|k| render[0].div_ceil(1 << k))
            .sum();
        assert!(width <= hiz_extent(render)[0]);
    }

    #[test]
    fn probe_grid_covers_the_render_size() {
        assert_eq!(grid([1707, 960], 16), [107, 60]);
        assert_eq!(grid([1280, 720], 8), [160, 90]);
    }
}
