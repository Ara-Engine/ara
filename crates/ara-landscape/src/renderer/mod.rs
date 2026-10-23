mod capture;
mod frame;
mod init;
mod record;
use crate::pages::{PAGE_CAPACITY, Pages, UPLOAD_BUDGET};
use crate::sample::LandscapeData;
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Device, Image, Pipeline, Queue, Resource, RootConstants,
    commands::{Attachment, Command, CommandList, PassTiming},
    cooked::CookedPipeline,
    vk,
    vulkan::VulkanDevice,
};
use ara_render::graph::{Graph, ResourceId, State};
use glam::{Mat4, Vec3};
use std::{collections::BTreeMap, path::Path};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    vp: Mat4,
    light_vp: Mat4,
    inverse_vp: Mat4,
    camera: [f32; 4],
    sun: [f32; 4],
    sun_color: [f32; 4],
    grid: [u32; 4],
    extent: [u32; 4],
    indices: [u32; 4],
    more: [u32; 4],
    params: [f32; 4],
    geometry: [u32; 4],
}

/// Explicit lighting/camera state; wind comes from a fixed presentation tick.
pub struct View {
    /// Eye in world metres.
    pub eye: Vec3,
    /// Camera aim point.
    pub target: Vec3,
    /// Direction toward the sun.
    pub sun: Vec3,
    /// Linear illuminant colour; intensity is scene-relative for this sample pass.
    pub sun_color: Vec3,
    /// Fixed tick driving deterministic wind.
    pub tick: u32,
    /// Seconds per tick.
    pub dt: f32,
    /// Vertical field of view in radians. Hosts compositing over another renderer
    /// use that renderer's camera, so reversed-Z depths compare directly.
    pub fov_y: f32,
    /// Near plane of the infinite reversed-Z projection, in metres.
    pub near: f32,
}
impl View {
    /// Noon view used by deterministic captures.
    pub fn noon() -> Self {
        Self {
            eye: Vec3::new(850., 43., 780.),
            target: Vec3::new(1070., 52., 1040.),
            sun: Vec3::new(-0.4, 0.8, -0.25).normalize(),
            sun_color: Vec3::new(1.0, 0.96, 0.86),
            tick: 120,
            dt: 1. / 60.,
            fov_y: 60f32.to_radians(),
            near: 0.5,
        }
    }
    /// Same camera/geometry with dusk lighting.
    pub fn dusk() -> Self {
        Self {
            sun: Vec3::new(-0.8, 0.14, -0.3).normalize(),
            sun_color: Vec3::new(1.5, 0.63, 0.28),
            ..Self::noon()
        }
    }
}

/// Native landscape pass owner. Creation/upload occurs off the editor frame path.
/// `record` only records; the host owns submission and completion without waiting.
pub struct Landscape {
    frame: Buffer,
    samples: Buffer,
    instances: Buffer,
    levels: Buffer,
    lists: Buffer,
    args: Buffer,
    upload: Buffer,
    visible: Buffer,
    foliage_args: Buffer,
    parameters: Buffer,
    slots: [u32; 9],
    pipelines: BTreeMap<String, Pipeline>,
    color: Image,
    depth: Image,
    shadow: Image,
    hzb: Image,
    image_slots: [u32; 4],
    textures: Vec<Image>,
    size: [u32; 2],
    count: u32,
    side: u32,
    cells: u32,
    tiles: u32,
    spacing: f32,
    previous: Vec<State>,
    pages: Pages,
    upload_slot: u32,
    pending_uploads: u32,
    upload_words: Vec<u32>,
    geometry: Option<Buffer>,
    geometry_slot: u32,
}

/// Public graph resource seam for integration with lighting, GI and editor captures.
pub struct Outputs {
    /// Tone-mapped, display-encoded SDR target; the host may present/copy it.
    pub color: Image,
    /// Reversed-Z terrain and alpha-tested vegetation depth.
    pub depth: Image,
    /// Directional terrain/foliage shadow map.
    pub shadow: Image,
    /// Conservative minimum-depth HZB of current terrain.
    pub hzb: Image,
}

/// Diagnostic GPU culling counters; these never feed runtime draw submission.
#[derive(serde::Serialize)]
pub struct VisibilityStats {
    /// Number of emitted terrain patches for each of the six detail levels.
    pub terrain_patches: [u32; 6],
    /// Emitted instances, species-major then LOD-minor; transition pairs count twice.
    pub foliage_instances: [u32; 9],
}
/// Timestamp distributions, grouped by graph pass with p50/p95/p99 in milliseconds.
pub fn timing_report(samples: &[Vec<PassTiming>]) -> serde_json::Value {
    let mut passes = BTreeMap::<String, Vec<f64>>::new();
    for frame in samples {
        for t in frame {
            passes
                .entry(t.pass.to_owned())
                .or_default()
                .push(t.milliseconds);
        }
    }
    serde_json::Value::Object(passes.into_iter().map(|(name,mut v)|{
        v.sort_by(f64::total_cmp);let n=v.len();(name,serde_json::json!({"p50_ms":v[n/2],"p95_ms":v[n*95/100],"p99_ms":v[n*99/100],"samples":n}))
    }).collect())
}
