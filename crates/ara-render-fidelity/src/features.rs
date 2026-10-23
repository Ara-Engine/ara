//! Completed-frame feature composition before the shared bloom/exposure/post stack.
use crate::{FidelityView, FrameInput, probes::ProbeVolume, targets::*};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Image, Queue, Resource, RootConstants, commands::Command, vulkan::VulkanDevice,
};
use ara_render::graph::{Graph, State};

/// Published same-device image and its actual completed access.
#[derive(Clone, Copy)]
pub struct ImageInput {
    /// GPU image handle.
    pub image: Image,
    /// Native sampled-image heap index.
    pub sampled_index: u32,
    /// Actual access/ownership; feature consumers return Graphics/ShaderRead.
    pub state: State,
}
/// Read-only scene inputs for particles and other transparent features.
pub struct FeatureInputs {
    /// Linear filtered radiance before exposure.
    pub radiance: ImageInput,
    /// Positive view distance in metres, zero for sky.
    pub depth: ImageInput,
    /// Completed directional DDGI storage; borrowed until the next render.
    pub probes: Buffer,
    /// Native storage-buffer heap index.
    pub probe_index: u32,
    /// Actual probe-buffer access/ownership.
    pub probe_state: State,
    /// Directional-probe lattice/packing contract.
    pub volume: ProbeVolume,
}
impl FidelityView {
    /// Borrow completed scene radiance/depth/probes for transparent feature draws.
    pub fn feature_inputs(&self) -> Result<FeatureInputs> {
        ensure!(self.history, "render before extracting feature inputs");
        let image = |i| ImageInput {
            image: self.targets.images[i],
            sampled_index: self.targets.sampled[i],
            state: self.targets.states[i],
        };
        let slot = 1 - self.probe_slot;
        Ok(FeatureInputs {
            radiance: image(FILTER0),
            depth: image(DEPTH),
            probes: self.probes[slot],
            probe_index: self.probe_indices[slot],
            probe_state: self.probe_states[slot],
            volume: self.volume,
        })
    }
    /// Run the actual fidelity bloom, exposure, lens, display and upscale pipelines
    /// over a feature-composited HDR input. This completed-frame seam resets display
    /// history; transparent motion/history integration can extend it independently.
    /// The input must match render_size and returns Graphics/ShaderRead.
    pub fn post_process(
        &mut self,
        gpu: &mut VulkanDevice,
        input: &FrameInput<'_>,
        colour: ImageInput,
    ) -> Result<()> {
        ensure!(
            self.history && colour.state.queue == Queue::Graphics,
            "post input requires a completed Graphics frame"
        );
        let (mut frame, _, _) = self.frame(input)?;
        frame.read[FILTER0 / 4][FILTER0 % 4] = colour.sampled_index;
        frame.temporal[0] = 1.;
        frame.control[0] = (1 - self.slot) as u32;
        gpu.upload(self.frame_buffer, bytemuck::bytes_of(&frame))?;
        let mut graph = Graph::default();
        let ids = self
            .targets
            .images
            .iter()
            .zip(&self.targets.states)
            .map(|(i, s)| graph.import(Resource::Image(*i), *s))
            .collect::<Vec<_>>();
        let source = self
            .targets
            .images
            .iter()
            .position(|image| *image == colour.image)
            .map(|i| ids[i])
            .unwrap_or_else(|| graph.import(Resource::Image(colour.image), colour.state));
        let fb = graph.import(
            Resource::Buffer(self.frame_buffer),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        let exposure = graph.import(Resource::Buffer(self.exposure), self.exposure_state);
        let lut = graph.import(
            Resource::Image(self.lut),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let size = self.settings.render_size;
        let display = self.settings.display_size;
        let groups = [size[0].div_ceil(8), size[1].div_ceil(8), 1];
        let display_groups = [display[0].div_ceil(8), display[1].div_ceil(8), 1];
        let mut recipes = Vec::new();
        let fsr = u32::from(self.settings.upscaler == crate::Upscaler::Fsr1 && size != display);
        for (name, pipeline, arg, dispatch, mut uses) in [
            (
                "Feature.Bloom.Horizontal",
                "bloom",
                0,
                groups,
                vec![(source, Access::ShaderRead), (ids[BLOOM0], Access::Storage)],
            ),
            (
                "Feature.Bloom.Vertical",
                "bloom",
                1,
                groups,
                vec![
                    (ids[BLOOM0], Access::ShaderRead),
                    (ids[BLOOM1], Access::Storage),
                ],
            ),
            (
                "Feature.Exposure",
                "exposure",
                0,
                [1, 1, 1],
                vec![(source, Access::ShaderRead), (exposure, Access::Storage)],
            ),
            (
                "Feature.Post",
                "post",
                0,
                groups,
                vec![
                    (source, Access::ShaderRead),
                    (ids[BLOOM1], Access::ShaderRead),
                    (ids[DEPTH], Access::ShaderRead),
                    (ids[MOTION], Access::ShaderRead),
                    (ids[POST], Access::Storage),
                    (exposure, Access::ShaderRead),
                    (lut, Access::ShaderRead),
                ],
            ),
            (
                "Feature.Upscale",
                "upscale",
                fsr,
                display_groups,
                vec![
                    (ids[POST], Access::ShaderRead),
                    (ids[UPSCALED], Access::Storage),
                    (ids[DISPLAY_HISTORY0 + 1 - self.slot], Access::Storage),
                ],
            ),
            (
                "Feature.Output",
                "sharpen",
                fsr,
                display_groups,
                vec![
                    (ids[UPSCALED], Access::ShaderRead),
                    (ids[OUTPUT], Access::Storage),
                ],
            ),
        ] {
            uses.push((fb, Access::ShaderRead));
            if pipeline == "post" && self.atmosphere.is_some() {
                uses.extend(
                    [SKYVIEW, TRANSMITTANCE, FROXELS, WORLD0 + 1 - self.slot]
                        .map(|i| (ids[i], Access::ShaderRead)),
                );
            }
            if pipeline == "upscale" {
                uses.extend(
                    [MOTION, DEPTH, WORLD0 + 1 - self.slot].map(|i| (ids[i], Access::ShaderRead)),
                );
            }
            let id = graph.pass(name, Queue::Graphics, uses);
            recipes.push((id, pipeline, arg, dispatch));
        }
        graph.pass(
            "Feature.Ready",
            Queue::Graphics,
            [(ids[OUTPUT], Access::CopySource)],
        );
        let compiled = graph.compile()?;
        for pass in &compiled.passes {
            let mut c = gpu.begin(Queue::Graphics);
            c.barriers(pass.before.iter().copied());
            c.push(Command::BeginPass(pass.name.to_owned()));
            if let Some((_, pipeline, arg, dispatch)) =
                recipes.iter().find(|(id, _, _, _)| *id == pass.id)
            {
                c.push(Command::Pipeline(self.pipelines[pipeline]));
                c.push(Command::Constants(RootConstants {
                    words: [self.frame_index, *arg, 0, 0, 0, 0, 0, 0],
                }));
                c.push(Command::Dispatch(*dispatch));
            }
            c.push(Command::EndPass);
            c.barriers(pass.after.iter().copied());
            let point = gpu.submit(c.finish(), &[])?;
            self.timings.extend(gpu.resolve(point)?);
        }
        for (i, id) in ids.iter().enumerate() {
            self.targets.states[i] = compiled.final_states[id.0];
        }
        self.exposure_state = compiled.final_states[exposure.0];
        gpu.assert_clean()?;
        Ok(())
    }
}
