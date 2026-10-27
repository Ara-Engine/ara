//! Reusable baseline exposure/ACES/sRGB pass for alternate HDR render paths.
use super::frame::Frame;
use anyhow::{Result, ensure};
use ara_gpu::{
    Buffer, Device, Pipeline, Resource, RootConstants, vk,
    vulkan::{Command, Recorder, VulkanDevice},
};
use bytemuck::Zeroable;

/// Display curve applied after exposure by the baseline tone map pass. The values
/// match `ara_render_fidelity::ToneMapper` so both renderers share one look.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ToneCurve {
    /// Narkowicz's per-channel ACES fit (the M1 curve).
    #[default]
    AcesFit = 0,
    /// Khronos PBR Neutral.
    Neutral = 1,
    /// Hill's fitted ACES RRT+ODT, pre-scaled to keep 18% grey.
    Filmic = 2,
}

/// `tonemapMain`'s pass-through view ID (`Frame.temporal.z`): outside every
/// `ViewMode` ID, so neither a debug view nor the tone curve applies (sRGB only).
const PASS_THROUGH: u32 = u32::MAX;

/// Shared offline-cooked baseline post pass; it owns no scene or gameplay state.
pub struct ToneMap {
    pipeline: Pipeline,
    frame: Buffer,
    frame_index: u32,
}
impl ToneMap {
    /// Loads the main renderer's cooked tonemap; never compiles at runtime.
    pub fn new(gpu: &mut VulkanDevice) -> Result<Self> {
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let metadata = serde_json::from_slice(&std::fs::read(artifacts.join("tonemap.json"))?)?;
        let pipeline = gpu.create_pipeline(&metadata, &artifacts)?;
        let frame = gpu.create_buffer(
            std::mem::size_of::<Frame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        Ok(Self {
            pipeline,
            frame,
            frame_index,
        })
    }
    /// Graph input: import as HostWrite after prepare, then declare ShaderRead.
    pub fn frame_buffer(&self) -> Buffer {
        self.frame
    }
    /// Uploads the private baseline ABI, keeping alternate renderers independent of it.
    /// Disable tone mapping for display-space diagnostic colors; sRGB remains applied.
    pub fn prepare(
        &self,
        gpu: &VulkanDevice,
        size: [u32; 2],
        exposure: f32,
        output: u32,
        tone_map: bool,
    ) -> Result<()> {
        ensure!(
            !size.contains(&0) && exposure.is_finite() && exposure > 0.,
            "Invalid post view/exposure"
        );
        let mut frame = Frame::zeroed();
        frame.extent = [size[0], size[1], 0, 0];
        frame.sun_color_exposure[3] = exposure;
        frame.post[3] = output;
        frame.temporal[2] = if tone_map { 0 } else { PASS_THROUGH };
        gpu.upload(self.frame, bytemuck::bytes_of(&frame))?;
        Ok(())
    }
    /// Records the pass. Declare HDR sampled input and output Storage in the graph.
    pub fn record(&self, recorder: &mut Recorder, hdr: u32, size: [u32; 2]) {
        recorder.push(Command::Pipeline(self.pipeline));
        recorder.push(Command::Constants(RootConstants {
            words: [self.frame_index, hdr, 0, 0, 0, 0, 0, 0],
        }));
        recorder.push(Command::Dispatch([
            size[0].div_ceil(8),
            size[1].div_ceil(8),
            1,
        ]));
    }
    /// Retires resources after their last submitted use when replacing a render path.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire_pipeline(self.pipeline)?;
        gpu.retire(Resource::Buffer(self.frame))?;
        Ok(())
    }
}
