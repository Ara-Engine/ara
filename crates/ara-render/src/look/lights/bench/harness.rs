//! The bench's GPU side: a raster-only device, the case's trace() scene, a G-buffer in
//! the renderer's encodings drawn by primary rays (`lightsPrimaryMain`), the raster
//! frame and look records the V2 passes read, their outputs and readbacks.
use super::scene::{self, Layout, MASK_BLOCKERS, MASK_FLOOR, Placed};
use crate::look::frame::LookFrame;
use crate::look::lights::Settings;
use crate::look::lights::gpu::{Capacity, Inputs, LightsGpu, Stats, artifacts, load};
use crate::look::ltc::LtcTables;
use crate::look::trace::{NO_MATERIAL, TraceGpu, TraceInstance, TraceMesh, TraceScene};
use crate::scene::Light;
use anyhow::Result;
use ara_gpu::commands::{Command, Recorder};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Pipeline, Queue, Resource, RootConstants, vk,
};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::sync::Arc;

/// `shaders/raster.slangh` `Frame` (renderer/frame.rs), byte for byte.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameRecord {
    vp: [f32; 16],
    previous_vp: [f32; 16],
    inverse_vp: [f32; 16],
    shadows: [[f32; 16]; 4],
    camera: [f32; 4],
    forward: [f32; 4],
    sun_direction: [f32; 4],
    sun_color_exposure: [f32; 4],
    splits: [f32; 4],
    extent: [u32; 4],
    gbuffer: [u32; 4],
    extra: [u32; 4],
    lighting: [u32; 4],
    post: [u32; 4],
    temporal: [u32; 4],
    look: [u32; 4],
    sizes: [u32; 4],
    upscale: [f32; 4],
}
const _: () = assert!(std::mem::size_of::<FrameRecord>() == 672);

/// An image with sampled and storage views.
#[derive(Clone, Copy)]
pub(crate) struct Target {
    pub image: Image,
    pub sampled: u32,
    pub storage: u32,
    state: Access,
}

impl Target {
    fn new(gpu: &mut GpuDevice, size: [u32; 2], format: vk::Format) -> Result<Self> {
        let image = gpu.create_image(
            size[0],
            size[1],
            format,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        Ok(Self {
            image,
            sampled: gpu.sampled_image(image, false)?.index,
            storage: gpu.storage_image(image)?.index,
            state: Access::Undefined,
        })
    }

    fn to(&mut self, after: Access) -> Barrier {
        let before = self.state;
        self.state = after;
        Barrier {
            resource: Resource::Image(self.image),
            before,
            after,
            handoff: None,
        }
    }
}

/// The G-buffer images.
struct GBuffer {
    depth: Target,
    normal: Target,
    albedo: Target,
    material: Target,
}

/// Which visibility a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blockers {
    /// Every instance occludes.
    On,
    /// The boxes are dropped from visibility (E3's no-blocker runs).
    Off,
}

impl Blockers {
    fn mask(self) -> u8 {
        match self {
            Self::On => MASK_FLOOR | MASK_BLOCKERS,
            Self::Off => MASK_FLOOR,
        }
    }
}

/// One V2 frame's measurements.
#[derive(Debug, Clone, Default)]
pub(crate) struct Timing {
    /// Pass label and milliseconds (Lights.Build, .Prep, .Bin, .Select, .Shade).
    pub passes: Vec<(String, f64)>,
    pub stats: Stats,
}

impl Timing {
    /// Milliseconds of the passes whose label starts with any of `labels`.
    pub(crate) fn ms(&self, labels: &[&str]) -> f64 {
        self.passes
            .iter()
            .filter(|(p, _)| labels.iter().any(|l| p == l))
            .map(|(_, ms)| ms)
            .sum()
    }
}

/// The bench device and one case's resources.
pub(crate) struct Bench {
    pub gpu: GpuDevice,
    pub size: [u32; 2],
    pub layout: Layout,
    primary: Pipeline,
    trace: TraceScene,
    trace_gpu: TraceGpu,
    lights: Buffer,
    lights_index: u32,
    light_capacity: u32,
    materials: Buffer,
    materials_index: u32,
    frame: Buffer,
    frame_index: u32,
    look: Buffer,
    look_index: u32,
    _ltc: LtcTables,
    gbuffer: GBuffer,
    /// RGBA32F radiance outputs: V2, exact control.
    pub v2: Target,
    pub exact: Target,
    /// Per-pixel counters (lit lights, rays, listed, omitted).
    pub counters: Target,
    /// The production-format radiance output (RGBA16F) timing runs write.
    pub half: Target,
    readback: Buffer,
    pub v2_gpu: LightsGpu,
    /// Frame-record camera position and view-projection.
    pub camera: Vec3,
    pub vp: Mat4,
    /// How many lights the frame holds.
    count: u32,
}

/// Bytes of one RGBA32F image.
fn image_bytes(size: [u32; 2]) -> u64 {
    u64::from(size[0]) * u64::from(size[1]) * 16
}

impl Bench {
    /// A device and the case's scene at `size`, with storage for `capacity` entries.
    pub(crate) fn new(layout: Layout, size: [u32; 2], entries: u32) -> Result<Self> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut gpu =
            GpuDevice::new_raster(&[], root.join("../../target/ara-render/lights-bench"))?;
        let primary = load(&mut gpu, &artifacts(), "lights_primary")?;
        let (positions, indices) = scene::cube();
        let blas = ara_bvh::Blas::build(&ara_bvh::Mesh {
            positions: &positions,
            indices: &indices,
            submeshes: &[],
        })?;
        let hash = blas.hash;
        let mut trace = TraceScene::default();
        trace.add_mesh(TraceMesh {
            blas: Arc::new(blas),
            uvs: None,
        });
        let first = layout.frame(0);
        for p in std::iter::once(layout.floor()).chain(first.boxes.iter().copied()) {
            trace.set_instance(p.id, instance(hash, p))?;
        }
        trace.commit();
        let trace_gpu = TraceGpu::new(&mut gpu, &trace)?;
        let light_capacity = layout.case.lights.max(1);
        let lights = gpu.create_buffer(
            u64::from(light_capacity) * std::mem::size_of::<Light>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let lights_index = gpu.storage_buffer(lights)?.index;
        let table: Vec<[f32; 8]> = scene::materials()
            .iter()
            .map(|m| {
                [
                    m.albedo[0],
                    m.albedo[1],
                    m.albedo[2],
                    m.roughness,
                    m.metal,
                    0.,
                    0.,
                    0.,
                ]
            })
            .collect();
        let materials = gpu.create_buffer(
            (table.len() * 32) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(materials, bytemuck::cast_slice(&table))?;
        let materials_index = gpu.storage_buffer(materials)?.index;
        let ltc = LtcTables::new(&mut gpu)?;
        let mut record: LookFrame = Zeroable::zeroed();
        record.light_info[0] = [ltc.sampled[0], ltc.sampled[1], 0, 0];
        let look = gpu.create_buffer(
            std::mem::size_of::<LookFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(look, bytemuck::bytes_of(&record))?;
        let look_index = gpu.storage_buffer(look)?.index;
        let frame = gpu.create_buffer(672, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        let f16 = vk::Format::R16G16B16A16_SFLOAT;
        let gbuffer = GBuffer {
            depth: Target::new(&mut gpu, size, vk::Format::R32_SFLOAT)?,
            normal: Target::new(&mut gpu, size, f16)?,
            albedo: Target::new(&mut gpu, size, f16)?,
            material: Target::new(&mut gpu, size, f16)?,
        };
        let f32x4 = vk::Format::R32G32B32A32_SFLOAT;
        let v2 = Target::new(&mut gpu, size, f32x4)?;
        let exact = Target::new(&mut gpu, size, f32x4)?;
        let counters = Target::new(&mut gpu, size, f32x4)?;
        let half = Target::new(&mut gpu, size, vk::Format::R16G16B16A16_SFLOAT)?;
        let readback = gpu.create_readback_buffer(image_bytes(size))?;
        let v2_gpu = LightsGpu::new(
            &mut gpu,
            Capacity {
                size,
                lights: light_capacity,
                coarse_lights: 1024,
                entries,
            },
        )?;
        let camera = layout.camera.resolve(size)?;
        let vp = camera.projection * camera.view;
        let mut bench = Self {
            gpu,
            size,
            layout,
            primary,
            trace,
            trace_gpu,
            lights,
            lights_index,
            light_capacity,
            materials,
            materials_index,
            frame,
            frame_index,
            look,
            look_index,
            _ltc: ltc,
            gbuffer,
            v2,
            exact,
            counters,
            half,
            readback,
            v2_gpu,
            camera: camera.position,
            vp,
            count: 0,
        };
        bench.write_frame_record(camera.forward)?;
        Ok(bench)
    }

    fn write_frame_record(&mut self, forward: Vec3) -> Result<()> {
        let g = &self.gbuffer;
        let mut f: FrameRecord = Zeroable::zeroed();
        f.vp = self.vp.to_cols_array();
        f.previous_vp = f.vp;
        f.inverse_vp = self.vp.inverse().to_cols_array();
        f.camera = self.camera.extend(1.).to_array();
        f.forward = forward.extend(0.).to_array();
        f.extent = [self.size[0], self.size[1], self.count, 0];
        f.gbuffer = [
            g.albedo.sampled,
            g.normal.sampled,
            g.material.sampled,
            g.albedo.sampled,
        ];
        f.extra = [g.depth.sampled, 0, 0, 0];
        f.lighting = [0, 0, 0, self.lights_index];
        f.look = [self.look_index, 0, 0, u32::MAX];
        f.sizes = [self.size[0], self.size[1], self.size[0], self.size[1]];
        self.gpu.upload(self.frame, bytemuck::bytes_of(&f))?;
        Ok(())
    }

    /// Poses frame `frame` of the case: lights, moved boxes (TLAS refit) and their
    /// upload. Call between completed submissions.
    pub(crate) fn pose(&mut self, frame: u32) -> Result<()> {
        let f = self.layout.frame(frame);
        for b in &f.boxes {
            self.trace.set_transform(b.id, b.world);
        }
        self.trace.commit();
        self.trace_gpu.update(&mut self.gpu, &self.trace)?;
        self.count = f.lights.len() as u32;
        anyhow::ensure!(self.count <= self.light_capacity, "light capacity");
        self.gpu
            .upload(self.lights, bytemuck::cast_slice(&f.lights))?;
        let forward = (self.layout_target() - self.camera).normalize();
        self.write_frame_record(forward)?;
        Ok(())
    }

    fn layout_target(&self) -> Vec3 {
        match self.layout.camera {
            crate::scene::CameraSpec::LookAt { target, .. } => target,
            _ => Vec3::ZERO,
        }
    }

    /// The V2 inputs for this frame.
    fn inputs(&self, output: u32, stats: bool, blockers: Blockers, settings: Settings) -> Inputs {
        Inputs {
            lights: self.count,
            output,
            stats: stats.then_some(self.counters.storage),
            trace: Some(self.trace_gpu.record()),
            mask: blockers.mask(),
            settings,
            footprint: scene::footprint(&self.layout.camera, self.size[1]),
        }
    }

    /// Records the primary pass into the G-buffer, leaving it readable.
    fn record_primary(&mut self, c: &mut Recorder) {
        let g = &mut self.gbuffer;
        c.barriers([
            g.depth.to(Access::Storage),
            g.normal.to(Access::Storage),
            g.albedo.to(Access::Storage),
            g.material.to(Access::Storage),
        ]);
        c.push(Command::BeginPass("Bench.Primary".into()));
        c.push(Command::Pipeline(self.primary));
        c.push(Command::Constants(RootConstants {
            words: [
                self.frame_index,
                0,
                self.materials_index,
                g.depth.storage,
                g.normal.storage,
                g.albedo.storage,
                g.material.storage,
                self.trace_gpu.record(),
            ],
        }));
        c.push(Command::Dispatch([
            self.size[0].div_ceil(8),
            self.size[1].div_ceil(8),
            1,
        ]));
        c.push(Command::EndPass);
        c.barriers([
            g.depth.to(Access::ShaderRead),
            g.normal.to(Access::ShaderRead),
            g.albedo.to(Access::ShaderRead),
            g.material.to(Access::ShaderRead),
        ]);
    }

    fn submit(&mut self, c: Recorder) -> Result<Vec<(String, f64)>> {
        let point = self.gpu.submit(c.finish(), &[])?;
        let timings = self.gpu.resolve(point)?;
        Ok(timings
            .into_iter()
            .map(|t| (t.pass, t.milliseconds))
            .collect())
    }

    /// Draws the posed frame's G-buffer and runs V2 into `v2` (with per-pixel
    /// counters when `stats`): its pass timings and counters.
    pub(crate) fn run_v2(
        &mut self,
        stats: bool,
        blockers: Blockers,
        settings: Settings,
    ) -> Result<Timing> {
        let inputs = self.inputs(self.v2.storage, stats, blockers, settings);
        self.v2_gpu.prepare(&mut self.gpu, &inputs)?;
        let mut c = self.gpu.begin(Queue::Graphics);
        self.record_primary(&mut c);
        let mut barriers = vec![self.v2.to(Access::Storage)];
        if stats {
            barriers.push(self.counters.to(Access::Storage));
        }
        c.barriers(barriers);
        self.v2_gpu.record(&mut c, self.frame_index, self.count);
        self.v2_gpu.record_stats_copy(&mut c);
        let passes = self.submit(c)?;
        Ok(Timing {
            passes,
            stats: self.v2_gpu.stats(&self.gpu)?,
        })
    }

    /// A timing frame: the G-buffer, then V2 into the production-format RGBA16F
    /// output, with visibility (`shadowed`) or without (shading cost alone).
    pub(crate) fn run_timed(&mut self, settings: Settings, shadowed: bool) -> Result<Timing> {
        let mut inputs = self.inputs(self.half.storage, false, Blockers::On, settings);
        if !shadowed {
            inputs.trace = None;
        }
        self.v2_gpu.prepare(&mut self.gpu, &inputs)?;
        let mut c = self.gpu.begin(Queue::Graphics);
        self.record_primary(&mut c);
        c.barriers([self.half.to(Access::Storage)]);
        self.v2_gpu.record(&mut c, self.frame_index, self.count);
        self.v2_gpu.record_stats_copy(&mut c);
        let passes = self.submit(c)?;
        Ok(Timing {
            passes,
            stats: self.v2_gpu.stats(&self.gpu)?,
        })
    }

    /// Draws the posed frame's G-buffer and runs the exact control into `exact`.
    pub(crate) fn run_exact(
        &mut self,
        stats: bool,
        blockers: Blockers,
        settings: Settings,
    ) -> Result<f64> {
        let inputs = self.inputs(
            self.exact.storage,
            stats,
            blockers,
            Settings {
                omission: None,
                ..settings
            },
        );
        self.v2_gpu.prepare(&mut self.gpu, &inputs)?;
        let mut c = self.gpu.begin(Queue::Graphics);
        self.record_primary(&mut c);
        let mut barriers = vec![self.exact.to(Access::Storage)];
        if stats {
            barriers.push(self.counters.to(Access::Storage));
        }
        c.barriers(barriers);
        self.v2_gpu.record_exact(&mut c, self.frame_index);
        let passes = self.submit(c)?;
        Ok(passes
            .iter()
            .filter(|(p, _)| p == "Lights.Exact")
            .map(|(_, ms)| ms)
            .sum())
    }

    /// Reads one RGBA32F (or 16F, widened) target back as floats, four per pixel.
    pub(crate) fn read(&mut self, which: Readback) -> Result<Vec<f32>> {
        let mut target = match which {
            Readback::V2 => self.v2,
            Readback::Exact => self.exact,
            Readback::Counters => self.counters,
            Readback::Material => self.gbuffer.material,
            Readback::Normal => self.gbuffer.normal,
            Readback::Depth => self.gbuffer.depth,
        };
        let mut c = self.gpu.begin(Queue::Graphics);
        let back = target.state;
        c.barriers([target.to(Access::CopySource)]);
        c.push(Command::ReadImage {
            source: target.image,
            destination: self.readback,
        });
        c.barriers([
            Barrier {
                resource: Resource::Buffer(self.readback),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            target.to(back),
        ]);
        self.submit(c)?;
        let bytes = self.gpu.read(self.readback)?;
        let pixels = self.size[0] as usize * self.size[1] as usize;
        if which == Readback::Depth {
            return Ok(bytemuck::cast_slice::<u8, f32>(&bytes[..pixels * 4]).to_vec());
        }
        Ok(if matches!(which, Readback::Material | Readback::Normal) {
            let pixels = self.size[0] as usize * self.size[1] as usize * 4;
            let halves: &[u16] = bytemuck::cast_slice(&bytes[..pixels * 2]);
            halves.iter().map(|&h| half_to_f32(h)).collect()
        } else {
            bytemuck::cast_slice::<u8, f32>(&bytes[..image_bytes(self.size) as usize]).to_vec()
        })
    }

    /// Releases the case's GPU storage before the device drops.
    pub(crate) fn destroy(self) -> Result<()> {
        let Self {
            mut gpu,
            trace_gpu,
            v2_gpu,
            lights,
            materials,
            frame,
            look,
            readback,
            gbuffer,
            v2,
            exact,
            counters,
            half,
            ..
        } = self;
        v2_gpu.destroy(&mut gpu)?;
        trace_gpu.destroy(&mut gpu)?;
        for b in [lights, materials, frame, look, readback] {
            gpu.retire(Resource::Buffer(b))?;
        }
        for t in [
            gbuffer.depth,
            gbuffer.normal,
            gbuffer.albedo,
            gbuffer.material,
            v2,
            exact,
            counters,
            half,
        ] {
            gpu.retire(Resource::Image(t.image))?;
        }
        gpu.collect()?;
        gpu.wait_idle()?;
        Ok(())
    }
}

/// What [`Bench::read`] copies back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readback {
    V2,
    Exact,
    Counters,
    /// The G-buffer material image (roughness, metal, occluder, material slot).
    Material,
    /// The G-buffer normal image (octahedral xy, RGBA16F widened).
    Normal,
    /// The G-buffer depth (reversed-Z, one float per pixel).
    Depth,
}

/// The trace instance of a placed cube.
fn instance(mesh: ara_bvh::MeshHash, p: Placed) -> TraceInstance {
    TraceInstance {
        mesh,
        world: p.world,
        mask: p.mask,
        materials: vec![NO_MATERIAL],
        user: p.material,
    }
}

/// IEEE half to f32.
pub(crate) fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1. } else { 1. };
    let exponent = i32::from((h >> 10) & 0x1f);
    let mantissa = f32::from(h & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1. + mantissa / 1024.) * 2f32.powi(e - 15),
    }
}
