//! Native output transform over a borrowed scene-linear BT.709 image. Fidelity
//! can supply its creative Neutral/AgX result without changing this pass's ABI.
use super::*;
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::{Barrier, vk};
/// Encoded offscreen image and tone-map pass for SDR, HDR10 PQ or linear scRGB.
pub struct DisplayOutput {
    input: Image,
    output: Image,
    pipeline: Pipeline,
    read: u32,
    write: u32,
    size: [u32; 2],
    settings: OutputSettings,
    state: Access,
}
impl DisplayOutput {
    /// Prepares a display target. Input must rest in Graphics/ShaderRead and
    /// contain exposed linear BT.709 (1 = diffuse white), with no SDR transfer.
    pub fn new(
        gpu: &mut VulkanDevice,
        input: Image,
        size: [u32; 2],
        settings: OutputSettings,
        cooked: &Path,
    ) -> Result<Self> {
        ensure!(
            settings.valid() && !size.contains(&0) && size.iter().all(|v| *v <= 8192),
            "invalid display settings"
        );
        let format = if settings.mode == OutputMode::Sdr {
            vk::Format::R8G8B8A8_UNORM
        } else {
            vk::Format::R16G16B16A16_SFLOAT
        };
        let output = gpu.create_image(
            size[0],
            size[1],
            format,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read = gpu.sampled_image(input, false)?.index;
        let write = gpu.storage_image(output)?.index;
        let manifest: CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join("display.json"))?)?;
        let pipeline = gpu.create_pipeline(&manifest, cooked)?;
        Ok(Self {
            input,
            output,
            pipeline,
            read,
            write,
            size,
            settings,
            state: Access::Undefined,
        })
    }
    /// Records highlight tone mapping and matching output encoding. HDR10
    /// converts primaries to BT.2020 before absolute PQ; scRGB retains negatives.
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
            "Display.ToneMapEncode",
            Queue::Graphics,
            [(source, Access::ShaderRead), (output, Access::Storage)],
        );
        g.pass(
            "Display.PresentReady",
            Queue::Graphics,
            [(output, Access::CopySource)],
        );
        let mut r = gpu.begin(Queue::Graphics);
        for pass in g.compile()?.passes {
            r.barriers(pass.before);
            if pass.name == "Display.ToneMapEncode" {
                r.push(Command::BeginPass(pass.name));
                r.push(Command::Pipeline(self.pipeline));
                r.push(Command::Constants(RootConstants {
                    words: [
                        self.read,
                        self.write,
                        self.size[0],
                        self.size[1],
                        self.settings.mode as u32,
                        self.settings.paper_white.to_bits(),
                        self.settings.peak_nits.to_bits(),
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
            r.barriers(pass.after);
        }
        self.state = Access::CopySource;
        Ok(r.finish())
    }
    /// Completed native presentation source, in CopySource after record.
    pub fn output(&self) -> Image {
        self.output
    }
    /// Exact selected encoding, for Present::display_encoded validation.
    pub fn settings(&self) -> OutputSettings {
        self.settings
    }
    /// Diagnostic readback of encoded values, preserving half-float HDR ranges.
    pub fn encoded_pixels(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        ensure!(
            self.state == Access::CopySource,
            "display output has not been recorded"
        );
        let stride = if self.settings.mode == OutputMode::Sdr {
            4
        } else {
            8
        };
        let read = gpu.create_buffer(
            u64::from(self.size[0]) * u64::from(self.size[1]) * stride,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut r = gpu.begin(Queue::Graphics);
        r.push(Command::ReadImage {
            source: self.output,
            destination: read,
        });
        r.barriers([Barrier {
            resource: Resource::Buffer(read),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let bytes = gpu.read(read)?;
        let pixels = if stride == 4 {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| v.map(|x| f32::from(x) / 255.))
                .collect()
        } else {
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|v| {
                    std::array::from_fn(|i| {
                        half::f16::from_bits(u16::from_le_bytes([v[i * 2], v[i * 2 + 1]])).to_f32()
                    })
                })
                .collect()
        };
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(pixels)
    }
    /// Retires this pass; the borrowed linear scene image stays host-owned.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.output))?;
        gpu.retire_pipeline(self.pipeline)?;
        gpu.collect()?;
        Ok(())
    }
}
