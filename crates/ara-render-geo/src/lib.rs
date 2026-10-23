#![warn(missing_docs)]
//! GPU-driven visibility and streamed virtual geometry over Ara's native RHI.
mod capture;
mod init;
mod record;
pub mod shadows;
pub mod streaming;
pub mod upscale;
use anyhow::{Result, ensure};
use ara_geo::Geometry;
use ara_gpu::{
    Access, Buffer, Device, Image, Pipeline, Queue, Resource, RootConstants,
    commands::{Attachment, Command, CommandList},
    cooked::CookedPipeline,
    vulkan::VulkanDevice,
};
use ara_render::{
    graph::{Graph, State},
    scene::CameraSpec,
};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::{collections::BTreeMap, path::Path};

/// Instance extraction ABI: uniform scale and translation, no gameplay state.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Instance {
    /// Mesh-local origin transformed to world, followed by positive scale.
    pub position_scale: [f32; 4],
    /// Linear base color and perceptual roughness.
    pub color_roughness: [f32; 4],
    /// Stable entity ID low/high words, reserved material ID, reserved.
    pub identity: [u32; 4],
}
/// Diagnostic views produced by the same visibility/resolve pipeline.
#[repr(u32)]
#[derive(Clone, Copy, Default)]
pub enum DebugView {
    /// Simple directional preview lighting from the resolved surface.
    #[default]
    Lit,
    /// Cluster identities.
    Clusters,
    /// Hierarchy levels.
    Lod,
    /// Raster attempts per pixel.
    Overdraw,
    /// White software triangles and blue hardware triangles.
    Raster,
}
/// Camera and selection policy for one extracted view.
pub struct View<'a> {
    /// Camera pose, using ara-render's reversed-Z projection convention.
    pub camera: &'a CameraSpec,
    /// Allowed geometric displacement in pixels; positive and finite.
    pub error_pixels: f32,
    /// Clear previous occlusion on a camera cut or deterministic capture.
    pub reset_history: bool,
    /// Diagnostic resolve mode.
    pub debug: DebugView,
    /// Route every triangle through hardware for a reference comparison.
    pub hardware_only: bool,
    /// Force leaf geometry for full-resolution error comparisons.
    pub full_resolution: bool,
}
/// Published completed surface channels for deferred lighting and capture.
#[derive(Clone, Copy)]
pub struct Outputs {
    /// Linear base color plus roughness.
    pub base_roughness: Image,
    /// World normal plus positive view distance (zero for background).
    pub normal_depth: Image,
    /// World position plus conservative local LOD displacement in world metres.
    pub world_error: Image,
    /// U64 instance/triangle identities as two U32 words per pixel.
    pub identities: Buffer,
    /// SDR diagnostic preview, without a fidelity claim.
    pub preview: Image,
    /// Every published channel is Graphics/ShaderRead after record.
    pub state: State,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Frame {
    vp: Mat4,
    inverse_vp: Mat4,
    previous_vp: Mat4,
    camera_error: [f32; 4],
    extent: [u32; 4],
    counts: [u32; 4],
    slots0: [u32; 4],
    slots1: [u32; 4],
    slots2: [u32; 4],
    slots3: [u32; 4],
    outputs: [u32; 4],
    min: [f32; 4],
    scale: [f32; 4],
    policy: [u32; 4],
    forward: [f32; 4],
    shadow: [u32; 4],
}
/// Per-view resources. Hosts own the device, frame completion and scene swaps.
pub struct GeometryView {
    frame: Buffer,
    buffers: Vec<Buffer>,
    slots: Vec<u32>,
    images: [Image; 4],
    depth: Image,
    image_slots: [u32; 4],
    pipelines: BTreeMap<String, Pipeline>,
    previous: Vec<State>,
    previous_vp: Mat4,
    size: [u32; 2],
    instance_count: u32,
    cluster_count: u32,
    group_count: u32,
    capacity: u32,
    min: [f32; 3],
    scale: [f32; 3],
    mesh: bool,
    last: Option<ara_gpu::TimelinePoint>,
    source_triangles: u64,
    residency: streaming::Residency,
}
/// Preparation budgets for a GPU visibility view.
pub struct GeometryConfig {
    /// Internal render size, independent of display reconstruction.
    pub size: [u32; 2],
    /// Maximum visible clusters across both occlusion phases.
    pub visible_clusters: u32,
    /// Physical cluster-page capacity; roots must fit. Each page uses 2048 bytes.
    pub resident_pages: u32,
    /// Prefer the optional mesh-shader backend; otherwise use indirect vertices.
    pub mesh_shaders: bool,
}
impl GeometryView {
    /// Uploads a view record after the prior use of this frame slot completes.
    pub fn update(&mut self, gpu: &mut VulkanDevice, view: &View<'_>) -> Result<()> {
        ensure!(
            view.error_pixels.is_finite() && view.error_pixels > 0.0,
            "invalid pixel error"
        );
        if let Some(p) = self.last {
            ensure!(
                gpu.completed()?[p.queue.index()] >= p.value,
                "frame slot still in flight"
            );
        }
        let camera = view.camera.resolve(self.size)?;
        let vp = camera.projection * camera.view;
        let focal = camera.projection.y_axis.y.abs() * self.size[1] as f32 * 0.5;
        let s = &self.slots;
        let f = Frame {
            vp,
            inverse_vp: vp.inverse(),
            previous_vp: self.previous_vp,
            camera_error: [
                camera.position.x,
                camera.position.y,
                camera.position.z,
                view.error_pixels,
            ],
            extent: [
                self.size[0],
                self.size[1],
                self.size[0].div_ceil(16),
                self.size[1].div_ceil(16),
            ],
            counts: [
                self.instance_count,
                self.cluster_count,
                self.group_count,
                self.capacity,
            ],
            slots0: [s[0], s[1], s[2], s[3]],
            slots1: [s[4], s[5], s[6], s[7]],
            slots2: [s[8], s[9], s[10], s[11]],
            slots3: [
                s[12],
                self.residency.table_slot,
                self.residency.floor,
                self.residency.upload_slot,
            ],
            outputs: self.image_slots,
            min: [self.min[0], self.min[1], self.min[2], focal],
            scale: [self.scale[0], self.scale[1], self.scale[2], 0.0],
            policy: [
                u32::from(view.reset_history),
                view.debug as u32,
                u32::from(view.hardware_only),
                u32::from(view.full_resolution),
            ],
            forward: camera.forward.extend(0.0).to_array(),
            shadow: [self.residency.pending, 0, 0, 0],
        };
        gpu.upload(self.frame, bytemuck::bytes_of(&f))?;
        self.previous_vp = vp;
        Ok(())
    }
    /// Record submission completion so update cannot overwrite an in-flight slot.
    pub fn submitted(&mut self, point: ara_gpu::TimelinePoint) {
        self.last = Some(point);
    }
    /// Read-only surface outputs; valid after the recorded work has completed.
    pub fn outputs(&self) -> Outputs {
        Outputs {
            base_roughness: self.images[0],
            normal_depth: self.images[1],
            world_error: self.images[2],
            identities: self.buffers[11],
            preview: self.images[3],
            state: State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        }
    }
}
