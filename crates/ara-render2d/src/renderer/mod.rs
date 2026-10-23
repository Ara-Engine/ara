use crate::{
    Scene2d,
    batch::{self, Batch},
};
use anyhow::{Result, ensure};
use ara_2d::Camera2d;
use ara_gpu::{
    Access, Barrier, Buffer, Device, Image, Pipeline, Queue, Resource, RootConstants, vk,
    vulkan::{Attachment, Command, PassTiming, VulkanDevice},
};
use ara_render::{
    capture::{Capture, CaptureContext, CaptureRequest, ShaderMetadata, ViewMode},
    graph::{Graph, State},
    renderer::post::ToneMap,
    scene::CameraSpec,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod passes;
mod upload;
/// Persistent immutable extraction and per-view GPU targets. No gameplay state lives here.
pub struct Renderer2d {
    gpu: VulkanDevice,
    pipeline: Pipeline,
    post: ToneMap,
    instances: Buffer,
    instance_index: u32,
    frame: Buffer,
    frame_index: u32,
    images: Vec<Image>,
    targets: [Image; 3],
    states: [State; 3],
    hdr_index: u32,
    output_index: u32,
    readbacks: [Buffer; 2],
    batches: Vec<Batch>,
    size: [u32; 2],
    scene_frame: Vec<u8>,
    names: Vec<(u64, String)>,
    hash: String,
    shaders: ShaderMetadata,
    timings: Vec<PassTiming>,
    instance_count: usize,
}
impl Renderer2d {
    /// Device diagnostics and native texture presentation integration.
    pub fn gpu(&self) -> &VulkanDevice {
        &self.gpu
    }
    /// Final sRGB-encoded SDR image, in CopySource state after render.
    pub fn output(&self) -> Image {
        self.targets[2]
    }
    /// Uploaded sprite/tile instance count, before per-chunk view culling.
    pub fn instance_count(&self) -> usize {
        self.instance_count
    }
    /// Last measured GPU passes, including shared post and readback.
    pub fn timings(&self) -> &[PassTiming] {
        &self.timings
    }
    /// Renders a pixel-snapped orthographic frame without temporal jitter/history.
    pub fn render(&mut self, camera: &Camera2d, exposure: f32) -> Result<()> {
        self.render_mode(camera, exposure, ViewMode::Lit)
    }
    /// Returns the same PNG, marks, picking and provenance contract as the 3D renderer.
    pub fn capture(&mut self, camera: &Camera2d, mut request: CaptureRequest) -> Result<Capture> {
        ensure!(
            request.size == self.size && request.settle_frames <= 1024,
            "capture size must match view; settle limit is 1024"
        );
        let (scale, centre) = camera.resolved()?;
        request.camera = CameraSpec::Orthographic {
            centre: glam::Vec2::from_array(centre),
            world_size: glam::Vec2::new(self.size[0] as f32 / scale, self.size[1] as f32 / scale),
        };
        for _ in 0..request.settle_frames.max(1) {
            self.render_mode(camera, request.exposure, request.mode)?;
        }
        let rgba = self.gpu.read(self.readbacks[0])?;
        let ids = self
            .gpu
            .read(self.readbacks[1])?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b) as u32)
            .collect();
        let mut hash = blake3::Hasher::new();
        hash.update(self.hash.as_bytes());
        hash.update(&serde_json::to_vec(camera)?);
        Capture::from_render(
            rgba,
            ids,
            &request,
            CaptureContext {
                names: &self.names,
                hash: hash.finalize().to_hex().as_ref(),
                device: &self.gpu.context().info,
                timings: &self.timings,
                shaders: &self.shaders,
            },
        )
    }
}
