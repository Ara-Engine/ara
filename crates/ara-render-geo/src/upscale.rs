//! Explicit linear-light bilinear reconstruction for the standalone benchmark.
//! Fidelity's temporal/FSR reconstruction should consume the exported surfaces.
use super::*;
use ara_gpu::{Barrier, vk};
/// Standalone spatial reconstruction. This does not claim FSR or temporal AA.
pub struct SpatialUpscaler {
    input: Image,
    output: Image,
    pipeline: Pipeline,
    read: u32,
    write: u32,
    input_size: [u32; 2],
    size: [u32; 2],
    state: Access,
}
impl SpatialUpscaler {
    /// Creates a display-sized diagnostic target for a completed preview image.
    pub fn new(
        gpu: &mut VulkanDevice,
        input: Image,
        input_size: [u32; 2],
        size: [u32; 2],
        cooked: &Path,
    ) -> Result<Self> {
        ensure!(
            !input_size.contains(&0) && !size.contains(&0) && size.iter().all(|v| *v <= 8192),
            "invalid upscale extent"
        );
        let output = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read = gpu.sampled_image(input, false)?.index;
        let write = gpu.storage_image(output)?.index;
        let manifest: CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join("upscale.json"))?)?;
        let pipeline = gpu.create_pipeline(&manifest, cooked)?;
        Ok(Self {
            input,
            output,
            pipeline,
            read,
            write,
            input_size,
            size,
            state: Access::Undefined,
        })
    }
    /// Records reconstruction; the caller submits on Graphics after visibility/VSM.
    pub fn record(&mut self, gpu: &VulkanDevice) -> Result<CommandList> {
        let mut g = Graph::default();
        let source = g.import(
            Resource::Image(self.input),
            State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        );
        let output = g.import(
            Resource::Image(self.output),
            State {
                access: self.state,
                queue: Queue::Graphics,
            },
        );
        g.pass(
            "Geo.Upscale.Bilinear",
            Queue::Graphics,
            [(source, Access::ShaderRead), (output, Access::Storage)],
        );
        g.pass(
            "Geo.Upscale.Publish",
            Queue::Graphics,
            [(output, Access::ShaderRead)],
        );
        let mut r = gpu.begin(Queue::Graphics);
        for p in g.compile()?.passes {
            r.barriers(p.before);
            if p.name == "Geo.Upscale.Bilinear" {
                r.push(Command::BeginPass(p.name));
                r.push(Command::Pipeline(self.pipeline));
                r.push(Command::Constants(RootConstants {
                    words: [
                        self.read,
                        self.write,
                        self.input_size[0],
                        self.input_size[1],
                        self.size[0],
                        self.size[1],
                        0,
                        0,
                    ],
                }));
                r.push(Command::Dispatch([
                    self.size[0].div_ceil(8),
                    self.size[1].div_ceil(8),
                    1,
                ]));
                r.push(Command::EndPass);
            }
            r.barriers(p.after);
        }
        self.state = Access::ShaderRead;
        Ok(r.finish())
    }
    /// Saves the completed display image through a diagnostic-only readback.
    pub fn capture(&self, gpu: &mut VulkanDevice, path: &Path) -> Result<()> {
        let read = gpu.create_buffer(
            u64::from(self.size[0]) * u64::from(self.size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([Barrier {
            resource: Resource::Image(self.output),
            before: Access::ShaderRead,
            after: Access::CopySource,
            handoff: None,
        }]);
        r.push(Command::ReadImage {
            source: self.output,
            destination: read,
        });
        r.barriers([
            Barrier {
                resource: Resource::Image(self.output),
                before: Access::CopySource,
                after: Access::ShaderRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
        ]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        image::save_buffer(
            path,
            &gpu.read(read)?,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
        )?;
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(())
    }
    /// Retires owned display resources after the host's last completed use.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.output))?;
        gpu.retire_pipeline(self.pipeline)?;
        gpu.collect()?;
        Ok(())
    }
}
