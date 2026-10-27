//! Screen-space refraction for transmissive forward surfaces (look-refract.slang,
//! look-refract.slangh). Frames whose forward pass draws a transmissive material
//! (the native culling's `refract_rect`) copy the lit opaque scene after deferred
//! lighting into an atlas: level 0 at the render size, then four blurred levels
//! beneath it for rough (frosted) glass, each only inside the screen rect the
//! refractive draws sample. Frames without one declare no pass and allocate nothing;
//! the atlas is allocated on the first frame that needs it and kept at the render
//! size (one sampled and one storage slot of the shared heap).
use super::{frame::LookFrame, gpu::LookGpu, passes::Frames};
use crate::graph::{Graph, PassId, ResourceId, State};
use anyhow::Result;
use ara_gpu::{
    Access, Device, GpuDevice, Image, Queue, Resource, RootConstants, vk,
    vulkan::{Command, Recorder},
};

/// Blurred levels beneath level 0 (each half the previous size).
pub(crate) const LEVELS: u32 = 4;
/// Linear march steps of a refracted ray against the copied depth.
const MARCH_STEPS: f32 = 16.;

/// The copied opaque scene: rgb radiance, a view depth (m; zero without a surface).
pub(crate) struct Refraction {
    image: Image,
    sampled: u32,
    storage: u32,
    state: State,
    /// The render size the atlas was allocated for.
    size: [u32; 2],
}

/// Atlas extent for render size `size`: level 0, and levels 1..4 side by side
/// beneath it (look-refract.slangh `refractLevelOrigin`).
fn extent([w, h]: [u32; 2]) -> [u32; 2] {
    [w, h + (h / 2).max(1)]
}

/// Level `level`'s texel rect (x0, y0, x1, y1) covering render-pixel rect `rect`.
fn level_rect(rect: [u32; 4], level: u32) -> [u32; 4] {
    let round_up = (1 << level) - 1;
    [
        rect[0] >> level,
        rect[1] >> level,
        (rect[2] + round_up) >> level,
        (rect[3] + round_up) >> level,
    ]
}

impl LookGpu {
    /// Prepares this frame's refraction of render-pixel rect `rect`: allocates the
    /// atlas (or reallocates it after a resize) and writes the look record's
    /// `refract_*` rows (shaders/look-refract.slangh).
    pub(crate) fn refraction(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &mut LookFrame,
        rect: [u32; 4],
    ) -> Result<()> {
        if self.refract.as_ref().is_some_and(|r| r.size != self.render)
            && let Some(old) = self.refract.take()
        {
            gpu.retire(Resource::Image(old.image))?;
        }
        if self.refract.is_none() {
            let [width, height] = extent(self.render);
            let image = gpu.create_image(
                width,
                height,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            self.refract = Some(Refraction {
                image,
                sampled: gpu.sampled_image(image, false)?.index,
                storage: gpu.storage_image(image)?.index,
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
                size: self.render,
            });
        }
        let r = self.refract.as_ref().expect("allocated above");
        frame.refract_info = [[r.sampled, r.storage, LEVELS, 0], rect];
        frame.refract_params = [[MARCH_STEPS, 0., 0., 0.], [0.; 4]];
        Ok(())
    }

    /// Declares the copy and its blurred levels after deferred lighting: `hdr` and
    /// `depth` are read, the atlas written. Returns the atlas the forward pass
    /// reads, with the passes and their levels to record.
    pub(crate) fn refract_passes(
        &self,
        graph: &mut Graph,
        look_frame: ResourceId,
        hdr: ResourceId,
        depth: ResourceId,
        reads: &[(ResourceId, Access)],
    ) -> Option<(ResourceId, Vec<(PassId, u32)>)> {
        let r = self.refract.as_ref()?;
        let atlas = graph.import(Resource::Image(r.image), r.state);
        const NAMES: [&str; LEVELS as usize + 1] = [
            "Refract.Copy",
            "Refract.Down1",
            "Refract.Down2",
            "Refract.Down3",
            "Refract.Down4",
        ];
        let passes = (0..=LEVELS)
            .map(|level| {
                let id = graph.pass(
                    NAMES[level as usize],
                    Queue::Graphics,
                    [
                        (hdr, Access::ShaderRead),
                        (depth, Access::ShaderRead),
                        (atlas, Access::Storage),
                        (look_frame, Access::ShaderRead),
                    ]
                    .into_iter()
                    .chain(reads.iter().copied()),
                );
                (id, level)
            })
            .collect();
        Some((atlas, passes))
    }

    /// Records level `level` over render-pixel rect `rect`: 0 copies the lit scene
    /// (sampled HDR `hdr`) and view depth, 1..4 each blur the level above.
    pub(crate) fn record_refract(
        &self,
        recorder: &mut Recorder,
        level: u32,
        frames: Frames,
        hdr: u32,
        rect: [u32; 4],
    ) {
        let [x0, y0, x1, y1] = level_rect(rect, level);
        recorder.push(Command::Pipeline(self.pipelines.refract));
        recorder.push(Command::Constants(RootConstants {
            words: [frames.render.0, hdr, level, 0, 0, 0, 0, 0],
        }));
        recorder.push(Command::Dispatch([
            (x1 - x0).div_ceil(8),
            (y1 - y0).div_ceil(8),
            1,
        ]));
    }

    /// Records the atlas' final state of a compiled frame.
    pub(crate) fn refract_finish(&mut self, atlas: ResourceId, final_states: &[State]) {
        if let Some(r) = &mut self.refract {
            r.state = final_states[atlas.0];
        }
    }
}

impl Refraction {
    /// Retires the atlas of a released view.
    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_cover_the_rect_and_pack_beneath_level_zero() {
        assert_eq!(extent([2560, 1440]), [2560, 2160]);
        assert_eq!(level_rect([10, 20, 101, 63], 0), [10, 20, 101, 63]);
        assert_eq!(level_rect([10, 20, 101, 63], 2), [2, 5, 26, 16]);
        // Levels 1..4 side by side fit within level 0's width.
        let w = 2561u32;
        assert!((1..=LEVELS).map(|l| (w >> l).max(1)).sum::<u32>() <= w);
    }
}
