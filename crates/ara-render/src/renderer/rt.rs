//! Hybrid ray queries (D29): High/Epic on devices with hardware ray queries trace
//! deterministic reflection rays from the raster G-buffer against the native
//! content (shaders/look-rt.slangh, look-rt.slang). Other devices, lower presets and
//! legacy content keep the raster path: no pass is declared and no ray-query
//! pipeline is created (only RT devices load them).
//!
//! Acceleration: one BLAS per finest-LOD geometry entry the instances use, built
//! over the shared native pool (`GeometryGpu::triangles`, no copies) when first
//! needed and again after a committed skin pose; one TLAS over every instance
//! (finest LOD, world transform, `custom_index` = instance row), rebuilt as a
//! fresh structure at once for a new BLAS, and for motion (moved instances, skin
//! refreshes) at most every `TLAS_INTERVAL` frames (ara-gpu acceleration
//! structures are static: no refit). Builds are recorded in the frame
//! graph (`RT.Accel`, after the G-buffer), so frames never wait on them.
//!
//! Reflections: `RT.Reflect` traces one ray per render pixel below the roughness
//! cut (on-screen hits take this frame's screen-space ambient light);
//! `RT.Filter.X`/`Y` widen glossy results by a Gaussian aware of the reflector's
//! depth and normal and of the reflected hit distance, and composite them over
//! screen-space reflections into the image the
//! `lighting_high_rt` variant reads (`ibl`'s `rays`). Below the cut traced
//! reflections own the pixel (screen-space confidence edges are half resolution and
//! move with the jitter: they would flicker); screen-space ones fill only the band
//! where traced ones fade out towards the cut, and everything rougher.
use super::native::{Native, rays::RAY_MASKED};
use super::*;
use crate::graph::{Graph, PassId, ResourceId};
use anyhow::Context;
use ara_gpu::{
    AccelerationStructure, Barrier, RootConstants,
    acceleration::{AccelerationDesc, AccelerationInstance},
    commands::{Command, Recorder},
    vulkan::{PipelinePreparation, PipelinePreparationId},
};

/// Largest reflection blur sigma in render pixels (`Look.rtParams[0].w`): nine taps
/// at half-sigma steps cover two sigmas; wider lobes fade to probe reflections.
const MAX_SIGMA: f32 = 12.;
/// Ray-origin bias: metres, plus this many render pixels' footprint per metre.
const BIAS: f32 = 0.002;
const BIAS_PIXELS: f32 = 2.;
/// Skin BLAS rebuilds per refresh after committed poses (the rest wait their turn).
const SKIN_BUILDS: usize = 2;
/// Frames between TLAS refreshes for motion alone (moved instances, refreshed skin
/// BLASes): reflections of moving content lag at most this many frames minus one,
/// deterministically, instead of paying a fresh TLAS every frame (Outpost: 0.36 ms).
/// New or replaced BLASes and a missing TLAS still rebuild at once.
const TLAS_INTERVAL: u32 = 4;

/// One declared hybrid pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RtPass {
    /// BLAS and TLAS builds that this frame needs.
    Accel,
    /// One reflection ray per render pixel (`rtReflectMain`).
    Reflect,
    /// Rough lobes' occlusion, four rays per 2x2 pixels (`rtRoughMain`).
    Rough,
    /// The rough result's spatial filter, axis 0 (x) or 1 (y) (`rtRoughFilterMain`).
    RoughFilter(u32),
    /// Blur axis 0 (x) or 1 (y, composited with screen-space reflections).
    Filter(u32),
}

#[derive(Clone, Copy)]
struct Pipelines {
    reflect: Pipeline,
    filter: Pipeline,
    /// Deferred lighting reading the traced reflections (`lightingHighRtFragment`).
    lighting: Pipeline,
    /// Transmissive forward draws refracting through traced interfaces
    /// (`nativeForwardRefractRtFragment`, shaders/native-refract-rt.slang).
    refract: Pipeline,
    /// Rough lobes' ray-traced occlusion (`rtRoughMain`).
    rough: Pipeline,
    /// Its spatial filter (`rtRoughFilterMain`).
    rough_filter: Pipeline,
}

struct Blas {
    structure: AccelerationStructure,
    /// Vertex and index buffers it is built from (a new skin slot rebuilds it).
    inputs: [Buffer; 2],
    built: bool,
}

struct Tlas {
    structure: AccelerationStructure,
    heap: u32,
    built: bool,
}

/// The traced (then composited) image and the blur's intermediate at the largest
/// render size seen, and the rough lobes' occlusion at half of it; RGBA16F, shared
/// by every camera (no history).
struct Images {
    size: [u32; 2],
    images: [Image; 4],
    sampled: [u32; 4],
    storage: [u32; 4],
    states: [State; 4],
}

/// The published generation the structures describe.
#[derive(Clone, Copy)]
struct Content {
    /// Its instance rows (a new buffer per generation).
    rows: Buffer,
    /// Its geometry buffers are acceleration-structure inputs.
    supported: bool,
}

/// This frame's graph identities, from `declare` until `finish`.
struct Ids {
    tlas: ResourceId,
    images: [ResourceId; 4],
}

/// Hybrid ray-query state of a renderer: acceleration structures over its native
/// content and the reflection images; shared by its cameras.
#[derive(Default)]
pub(super) struct Rays {
    psos: Psos,
    /// This frame's ready PSOs (`Some` whenever it traces).
    pipelines: Option<Pipelines>,
    content: Option<Content>,
    /// Instance -> finest-LOD geometry entry (u32 rows), per generation.
    entries: Option<(Buffer, u32)>,
    masked: bool,
    /// Per geometry entry.
    blas: Vec<Option<Blas>>,
    /// Replaced BLASes, retired once the TLAS that references them is.
    replaced: Vec<AccelerationStructure>,
    /// Skin geometry entries whose BLAS lags a committed pose, oldest first.
    skins: std::collections::VecDeque<u32>,
    /// Per instance, as the TLAS holds them (row-major 3x4).
    transforms: Vec<[f32; 12]>,
    tlas: Option<Tlas>,
    /// Frames passed without a sync: compare every instance next time.
    stale: bool,
    /// Frames since the TLAS was last rebuilt (`TLAS_INTERVAL`).
    since_rebuild: u32,
    /// Motion is waiting for the next interval refresh.
    pending: bool,
    images: Option<Images>,
    /// Bindless tables of this frame's content (`Some`: this frame traces).
    tables: Option<super::native::rays::RayTables>,
    ids: Option<Ids>,
}

/// Cooked recipes, in `Pipelines` field order.
const RECIPES: [&str; 6] = [
    "look_rt_reflect",
    "look_rt_filter",
    "lighting_high_rt",
    "native_forward_refract_rt",
    "look_rt_rough",
    "look_rt_rough_filter",
];

/// The ray-query PSOs, compiled off-thread (`GpuDevice::prepare_pipelines`) the
/// first time a frame wants them; frames trace once they are ready.
#[derive(Default)]
enum Psos {
    #[default]
    Empty,
    Pending(PipelinePreparationId),
    Ready(Pipelines),
    /// The cook lacks them (or they failed): every frame stays raster.
    Unavailable,
}

impl Psos {
    /// Ready PSOs, starting or polling their preparation; `wait` (deterministic
    /// captures) blocks until it ends so results never depend on compile timing.
    fn poll(
        &mut self,
        gpu: &mut GpuDevice,
        artifacts: &Path,
        key: &str,
        wait: bool,
    ) -> Option<Pipelines> {
        // A wait never outlives a stuck preparation worker.
        let started = std::time::Instant::now();
        loop {
            match self.step(gpu, artifacts, key) {
                Ok(Some(pipelines)) => return Some(pipelines),
                Ok(None)
                    if wait
                        && !gpu.deferring_completion()
                        && !matches!(self, Self::Unavailable)
                        && started.elapsed().as_secs() < 30 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(None) => return None,
                Err(error) => {
                    // An incomplete cook keeps the raster path rather than failing frames.
                    eprintln!("Ara: hybrid ray queries unavailable, raster path: {error:#}");
                    *self = Self::Unavailable;
                    return None;
                }
            }
        }
    }

    fn step(
        &mut self,
        gpu: &mut GpuDevice,
        artifacts: &Path,
        key: &str,
    ) -> Result<Option<Pipelines>> {
        if let Self::Empty = self {
            let mut recipes = Vec::with_capacity(RECIPES.len());
            for name in RECIPES {
                let cooked: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(
                    &std::fs::read(artifacts.join(format!("{name}.json")))?,
                )?;
                ensure!(
                    cooked.key == key,
                    "hybrid ray-query cook is incomplete; recook the pipeline manifest"
                );
                recipes.push(PipelinePreparation {
                    cooked,
                    artifacts: artifacts.to_owned(),
                });
            }
            // None: the preparation worker is busy (native PSOs); retry next frame.
            if let Some(ticket) = gpu.prepare_pipelines(|| recipes)? {
                *self = Self::Pending(ticket);
            }
        }
        if let Self::Pending(ticket) = *self
            && let Some(handles) = gpu.poll_prepared_pipelines(ticket)?
        {
            let [reflect, filter, lighting, refract, rough, rough_filter]: [Pipeline;
                RECIPES.len()] = handles
                .try_into()
                .map_err(|_| anyhow::anyhow!("hybrid PSO count differs"))?;
            *self = Self::Ready(Pipelines {
                reflect,
                filter,
                lighting,
                refract,
                rough,
                rough_filter,
            });
        }
        Ok(match self {
            Self::Ready(pipelines) => Some(*pipelines),
            _ => None,
        })
    }
}

/// Row-major 3x4 object-to-world rows of a column-major affine matrix.
fn affine(world: &Mat4) -> [f32; 12] {
    let w = world.to_cols_array_2d();
    [
        w[0][0], w[1][0], w[2][0], w[3][0], w[0][1], w[1][1], w[2][1], w[3][1], w[0][2], w[1][2],
        w[2][2], w[3][2],
    ]
}

impl Rays {
    /// Brings the structures up to date for this frame (call after the native
    /// prepare, before the frame record is written). Traces when the quality asks
    /// for ray queries, the device has them, native content is published and the
    /// PSOs are compiled (off-thread; `deterministic` captures wait for them).
    /// Primary frames that do not trace release everything (secondary ones keep it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        gpu: &mut GpuDevice,
        native: Option<&Native>,
        quality: &crate::look::Quality,
        render: [u32; 2],
        primary: bool,
        deterministic: bool,
        artifacts: &Path,
        key: &str,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Raster.rays_prepare");
        self.tables = None;
        if self.ids.take().is_some() {
            // The last frame declared builds but never finished: whether they ran
            // is unknown, so start over from fresh structures.
            self.release(gpu)?;
        }
        let native = native.filter(|native| {
            quality.ray_queries.is_some()
                && gpu.info().ray_tracing
                && native.ray_tables().instances > 0
        });
        let Some(native) = native else {
            if primary {
                self.release(gpu)?;
            } else {
                self.stale = true;
            }
            return Ok(());
        };
        // Until the PSOs are compiled, frames keep the raster path.
        self.pipelines = self.psos.poll(gpu, artifacts, key, deterministic);
        if self.pipelines.is_none() {
            return Ok(());
        }
        let tables = native.ray_tables();
        let synced = match self.sync(gpu, native) {
            Ok(synced) => synced,
            Err(error) => {
                // This generation stays raster (e.g. acceleration memory exhausted);
                // the next content generation tries again.
                eprintln!("Ara: hybrid ray queries off for this content: {error:#}");
                self.release(gpu)?;
                self.content = Some(Content {
                    rows: tables.rows,
                    supported: false,
                });
                false
            }
        };
        if synced {
            self.ensure_images(gpu, render)?;
            self.tables = Some(tables);
        }
        Ok(())
    }

    /// Updates BLASes and the TLAS for `native`. False when its geometry is not an
    /// acceleration-structure input (a seedless preparation): it stays raster.
    fn sync(&mut self, gpu: &mut GpuDevice, native: &Native) -> Result<bool> {
        let tables = native.ray_tables();
        let geometry = native.ray_geometry();
        if self.content.is_none_or(|c| c.rows != tables.rows) {
            self.release_structures(gpu)?;
            let mut entries = Vec::with_capacity(tables.instances);
            let mut masked = false;
            // TLAS custom indices (instance rows) have 24 bits.
            let mut supported = tables.instances <= 0xff_ffff;
            for i in 0..tables.instances {
                let (_, entry, mask) = native.ray_instance(i);
                supported &= geometry.triangles(entry).is_some();
                masked |= mask == RAY_MASKED;
                entries.push(entry);
            }
            self.content = Some(Content {
                rows: tables.rows,
                supported,
            });
            if !supported {
                eprintln!(
                    "Ara: native geometry was prepared without acceleration-structure inputs; hybrid ray queries are off for this content"
                );
                return Ok(false);
            }
            let buffer = gpu.create_buffer(
                (entries.len() * 4) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            self.entries = Some((buffer, 0));
            gpu.upload(buffer, bytemuck::cast_slice(&entries))?;
            self.entries = Some((buffer, gpu.storage_buffer(buffer)?.index));
            self.masked = masked;
            self.transforms = vec![[0.; 12]; tables.instances];
        }
        if !self.content.is_some_and(|c| c.supported) {
            return Ok(false);
        }
        // Without a TLAS, or after frames that skipped this sync, every instance is
        // checked; otherwise only the rows and skins this frame changed.
        let all = self.tlas.is_none() || self.stale;
        let (rows, skins) = native.ray_changes();
        // Structural changes (no TLAS yet, a new BLAS) rebuild at once; motion waits
        // for the interval refresh.
        let mut structural = self.tlas.is_none();
        let mut moved = false;
        let mut check = |rays: &mut Self, gpu: &mut GpuDevice, i: usize| -> Result<()> {
            let (world, entry, _) = native.ray_instance(i);
            structural |= rays.ensure_blas(gpu, native, entry)?;
            let transform = affine(world);
            if rays.transforms[i] != transform {
                rays.transforms[i] = transform;
                moved = true;
            }
            Ok(())
        };
        if all {
            for i in 0..tables.instances {
                check(self, gpu, i)?;
            }
        } else {
            for &i in rows.iter().chain(skins) {
                check(self, gpu, i)?;
            }
        }
        self.stale = false;
        self.since_rebuild = self.since_rebuild.saturating_add(1);
        let due = structural || self.since_rebuild >= TLAS_INTERVAL;
        // Skin BLAS builds stay off the per-frame path: a few per refresh, oldest
        // first, each over the latest committed pose.
        let mut refreshed = false;
        if due {
            for _ in 0..SKIN_BUILDS {
                let Some(entry) = self.skins.pop_front() else {
                    break;
                };
                self.build_blas(gpu, native, entry)?;
                refreshed = true;
            }
        }
        self.pending |= moved;
        if structural || (due && (self.pending || refreshed)) {
            self.rebuild_tlas(gpu, native)?;
            self.since_rebuild = 0;
            self.pending = false;
        }
        Ok(true)
    }

    /// A BLAS for geometry `entry` over its current vertices; true when created now.
    /// An existing skin BLAS whose pose changed only joins `skins`: rebuilds are
    /// amortised (`SKIN_BUILDS` per frame) and its hits meanwhile interpolate the
    /// current pose's attributes at the previous pose's barycentrics.
    fn ensure_blas(&mut self, gpu: &mut GpuDevice, native: &Native, entry: u32) -> Result<bool> {
        let range = native
            .ray_geometry()
            .triangles(entry)
            .context("native geometry entry without acceleration inputs")?;
        let slot = entry as usize;
        if self.blas.len() <= slot {
            self.blas.resize_with(slot + 1, || None);
        }
        match &self.blas[slot] {
            Some(b) if b.inputs == [range.vertices, range.indices] => Ok(false),
            Some(_) => {
                if !self.skins.contains(&entry) {
                    self.skins.push_back(entry);
                }
                Ok(false)
            }
            None => self.build_blas(gpu, native, entry),
        }
    }

    /// Creates the BLAS of `entry` over its current vertices (built in `RT.Accel`).
    fn build_blas(&mut self, gpu: &mut GpuDevice, native: &Native, entry: u32) -> Result<bool> {
        let range = native
            .ray_geometry()
            .triangles(entry)
            .context("native geometry entry without acceleration inputs")?;
        let structure =
            gpu.create_acceleration_structure(&AccelerationDesc::TriangleRange(range))?;
        let blas = Blas {
            structure,
            inputs: [range.vertices, range.indices],
            built: false,
        };
        if let Some(old) = self.blas[entry as usize].replace(blas) {
            self.replaced.push(old.structure);
        }
        Ok(true)
    }

    /// A fresh TLAS over every instance; the previous one and the BLASes only it
    /// referenced retire (their GPU lifetime ends after in-flight frames).
    fn rebuild_tlas(&mut self, gpu: &mut GpuDevice, native: &Native) -> Result<()> {
        ara_gpu::cpu_scope!("Raster.rays_tlas");
        let count = native.ray_tables().instances;
        let mut instances = Vec::with_capacity(count);
        for (i, &transform) in self.transforms.iter().enumerate().take(count) {
            let (_, entry, mask) = native.ray_instance(i);
            let geometry = self.blas[entry as usize]
                .as_ref()
                .context("instance geometry has a BLAS")?
                .structure;
            instances.push(AccelerationInstance {
                geometry,
                transform,
                custom_index: u32::try_from(i)?,
                mask,
            });
        }
        let structure =
            gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
        let heap = match gpu.bind_acceleration_structure(structure) {
            Ok(heap) => heap.index,
            Err(error) => {
                gpu.retire(Resource::AccelerationStructure(structure))?;
                return Err(error.into());
            }
        };
        let previous = self.tlas.replace(Tlas {
            structure,
            heap,
            built: false,
        });
        if let Some(previous) = previous {
            gpu.retire(Resource::AccelerationStructure(previous.structure))?;
        }
        for blas in self.replaced.drain(..) {
            gpu.retire(Resource::AccelerationStructure(blas))?;
        }
        gpu.collect()?;
        Ok(())
    }

    /// Retires every structure, the entry table and the images (presets or content
    /// without ray queries, device-side uncertainty after a failed frame).
    fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        self.release_structures(gpu)?;
        if let Some(images) = self.images.take() {
            for image in images.images {
                gpu.retire(Resource::Image(image))?;
            }
            gpu.collect()?;
        }
        self.tables = None;
        self.ids = None;
        Ok(())
    }

    /// Retires the TLAS, then the BLASes it referenced, and the entry table.
    fn release_structures(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let mut retired = false;
        if let Some(tlas) = self.tlas.take() {
            gpu.retire(Resource::AccelerationStructure(tlas.structure))?;
            retired = true;
        }
        for blas in self
            .replaced
            .drain(..)
            .chain(self.blas.drain(..).flatten().map(|b| b.structure))
        {
            gpu.retire(Resource::AccelerationStructure(blas))?;
            retired = true;
        }
        if let Some((buffer, _)) = self.entries.take() {
            gpu.retire(Resource::Buffer(buffer))?;
            retired = true;
        }
        self.content = None;
        self.transforms.clear();
        self.skins.clear();
        self.stale = false;
        if retired {
            gpu.collect()?;
        }
        Ok(())
    }

    fn ensure_images(&mut self, gpu: &mut GpuDevice, render: [u32; 2]) -> Result<()> {
        if self
            .images
            .as_ref()
            .is_some_and(|i| i.size[0] >= render[0] && i.size[1] >= render[1])
        {
            return Ok(());
        }
        let size = self.images.as_ref().map_or(render, |i| {
            [i.size[0].max(render[0]), i.size[1].max(render[1])]
        });
        if let Some(old) = self.images.take() {
            for image in old.images {
                gpu.retire(Resource::Image(image))?;
            }
        }
        // The trace and the blur's intermediate at full size, rough occlusion at half.
        let half = size.map(|v| v.div_ceil(2));
        let mut images = Vec::with_capacity(3);
        for extent in [size, size, half, half] {
            let image = gpu.create_image(
                extent[0],
                extent[1],
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            images.push(image);
        }
        let images = [images[0], images[1], images[2], images[3]];
        let mut sampled = [0; 4];
        let mut storage = [0; 4];
        for (i, &image) in images.iter().enumerate() {
            sampled[i] = gpu.sampled_image(image, false)?.index;
            storage[i] = gpu.storage_image(image)?.index;
        }
        self.images = Some(Images {
            size,
            images,
            sampled,
            storage,
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; 4],
        });
        Ok(())
    }

    /// Whether this frame traces (`prepare` found everything it needs).
    pub(super) fn active(&self) -> bool {
        self.tables.is_some()
    }

    /// Writes this frame's `rt_params` and `rt_info` rows of the look record
    /// (shaders/look-rt.slangh); frames that do not trace leave them zero (raster
    /// only). `projection_y` is the camera projection's y scale, `intensity` the
    /// look's reflection strength.
    pub(super) fn write_rows(
        &self,
        frame: &mut crate::look::frame::LookFrame,
        quality: &crate::look::Quality,
        projection_y: f32,
        render: [u32; 2],
        intensity: f32,
    ) -> Option<()> {
        let (tables, settings, tlas, entries, images) = (
            self.tables?,
            quality.ray_queries?,
            self.tlas.as_ref()?,
            self.entries?,
            self.images.as_ref()?,
        );
        // Radians per render pixel at the image centre.
        let spread = 2. / (projection_y.abs().max(1e-6) * render[1].max(1) as f32);
        // Enabled, alpha-tested instances, and whether on-screen hits take this frame's
        // screen-space ambient light (the High/Epic visibility bitmask, look::gpu
        // bitmask_ao, whose image layout shading reads).
        let screen_ambient = quality.high_tier && quality.ambient_occlusion.is_some();
        let flags = 1 | u32::from(self.masked) << 1 | u32::from(screen_ambient) << 2;
        frame.rt_params = [
            [
                settings.reflection_roughness,
                settings.distance,
                spread,
                MAX_SIGMA,
            ],
            [
                BIAS,
                BIAS_PIXELS * spread,
                intensity,
                settings.rough_roughness,
            ],
        ];
        frame.rt_info = [
            [
                tlas.heap,
                tables.rows_index,
                tables.geometry,
                tables.materials,
            ],
            [tables.textures, entries.1, images.sampled[0], flags],
        ];
        Some(())
    }

    /// Declares this frame's passes: structure builds after `gbuffer` (whose
    /// barriers publish skin poses to shader reads), then the trace and the two blur
    /// axes. `reads` must cover the G-buffer, screen-space reflections, the lighting
    /// caches (probe GI, sky, sun maps) and the content's bindless resources.
    pub(super) fn declare(
        &mut self,
        graph: &mut Graph,
        gbuffer: PassId,
        reads: &[(ResourceId, Access)],
    ) -> Result<Vec<(PassId, RtPass)>> {
        let (Some(tlas), Some(images)) = (&self.tlas, &self.images) else {
            return Ok(Vec::new());
        };
        if self.tables.is_none() {
            return Ok(Vec::new());
        }
        let ids = Ids {
            tlas: graph.import(
                Resource::AccelerationStructure(tlas.structure),
                State {
                    access: if tlas.built {
                        Access::AsRead
                    } else {
                        Access::Undefined
                    },
                    queue: Queue::Graphics,
                },
            ),
            images: [
                graph.import(Resource::Image(images.images[0]), images.states[0]),
                graph.import(Resource::Image(images.images[1]), images.states[1]),
                graph.import(Resource::Image(images.images[2]), images.states[2]),
                graph.import(Resource::Image(images.images[3]), images.states[3]),
            ],
        };
        let mut passes = Vec::new();
        let pending = !tlas.built || self.blas.iter().flatten().any(|b| !b.built);
        if pending {
            let accel = graph.pass(
                "RT.Accel",
                Queue::Graphics,
                [(ids.tlas, Access::AsBuild)]
                    .into_iter()
                    .chain(reads.iter().copied()),
            );
            graph.after(accel, gbuffer)?;
            passes.push((accel, RtPass::Accel));
        }
        let [trace, blur, rough, widened] = ids.images;
        let stages = [
            (
                "RT.Reflect",
                RtPass::Reflect,
                vec![(trace, Access::Storage)],
            ),
            ("RT.Rough", RtPass::Rough, vec![(rough, Access::Storage)]),
            (
                "RT.Rough.X",
                RtPass::RoughFilter(0),
                vec![(rough, Access::ShaderRead), (widened, Access::Storage)],
            ),
            (
                "RT.Rough.Y",
                RtPass::RoughFilter(1),
                vec![(widened, Access::ShaderRead), (rough, Access::Storage)],
            ),
            (
                "RT.Filter.X",
                RtPass::Filter(0),
                vec![(trace, Access::ShaderRead), (blur, Access::Storage)],
            ),
            (
                "RT.Filter.Y",
                RtPass::Filter(1),
                vec![
                    (blur, Access::ShaderRead),
                    (rough, Access::ShaderRead),
                    (trace, Access::Storage),
                ],
            ),
        ];
        for (name, pass, uses) in stages {
            let id = graph.pass(
                name,
                Queue::Graphics,
                uses.into_iter()
                    .chain([(ids.tlas, Access::AsRead)])
                    .chain(reads.iter().copied()),
            );
            passes.push((id, pass));
        }
        self.ids = Some(ids);
        Ok(passes)
    }

    /// What deferred lighting reads of this frame's traced reflections (empty when
    /// it does not trace).
    pub(super) fn lighting_reads(&self) -> Option<(ResourceId, Access)> {
        self.ids
            .as_ref()
            .map(|ids| (ids.images[0], Access::ShaderRead))
    }

    /// The TLAS read of any other pass that traces this frame (shaders including
    /// look-rt.slangh, e.g. ray-traced refraction); declare it after `declare`.
    pub(super) fn tlas_read(&self) -> Option<(ResourceId, Access)> {
        self.ids.as_ref().map(|ids| (ids.tlas, Access::AsRead))
    }

    /// The refractive forward PSO tracing its interfaces, on frames that trace.
    pub(super) fn refract(&self) -> Option<Pipeline> {
        self.ids.as_ref()?;
        self.pipelines.map(|p| p.refract)
    }

    /// The deferred lighting PSO reading traced reflections, on frames that trace.
    pub(super) fn lighting(&self) -> Option<Pipeline> {
        self.ids.as_ref()?;
        self.pipelines.map(|p| p.lighting)
    }

    /// Records a declared pass. `frame` is the scene frame record's bindless index
    /// and `render` its extent.
    pub(super) fn record(
        &self,
        recorder: &mut Recorder,
        pass: RtPass,
        frame: u32,
        render: [u32; 2],
    ) {
        let (Some(pipelines), Some(images)) = (self.pipelines, &self.images) else {
            return;
        };
        let groups = [render[0].div_ceil(8), render[1].div_ceil(8), 1];
        // The rough lobes' passes run at half the render size.
        let half = [
            render[0].div_ceil(2).div_ceil(8),
            render[1].div_ceil(2).div_ceil(8),
            1,
        ];
        match pass {
            RtPass::Accel => self.record_builds(recorder),
            RtPass::Reflect => {
                recorder.push(Command::Pipeline(pipelines.reflect));
                recorder.push(Command::Constants(RootConstants {
                    words: [frame, images.storage[0], 0, 0, 0, 0, 0, 0],
                }));
                recorder.push(Command::Dispatch(groups));
            }
            RtPass::Rough => {
                recorder.push(Command::Pipeline(pipelines.rough));
                recorder.push(Command::Constants(RootConstants {
                    words: [frame, images.storage[2], 0, 0, 0, 0, 0, 0],
                }));
                recorder.push(Command::Dispatch(half));
            }
            RtPass::RoughFilter(axis) => {
                let (input, output) = if axis == 0 { (2, 3) } else { (3, 2) };
                recorder.push(Command::Pipeline(pipelines.rough_filter));
                recorder.push(Command::Constants(RootConstants {
                    words: [
                        frame,
                        images.sampled[input],
                        images.storage[output],
                        axis,
                        0,
                        0,
                        0,
                        0,
                    ],
                }));
                recorder.push(Command::Dispatch(half));
            }
            RtPass::Filter(axis) => {
                let (input, output) = if axis == 0 { (0, 1) } else { (1, 0) };
                recorder.push(Command::Pipeline(pipelines.filter));
                recorder.push(Command::Constants(RootConstants {
                    words: [
                        frame,
                        images.sampled[input],
                        images.storage[output],
                        axis,
                        images.sampled[2],
                        0,
                        0,
                        0,
                    ],
                }));
                recorder.push(Command::Dispatch(groups));
            }
        }
    }

    /// Pending BLAS builds (their vertex/index inputs move from shader reads to
    /// build reads first), then the TLAS over them.
    fn record_builds(&self, recorder: &mut Recorder) {
        let edge = |resource, before, after| Barrier {
            resource,
            before,
            after,
            handoff: None,
        };
        let pending: Vec<_> = self.blas.iter().flatten().filter(|b| !b.built).collect();
        // A handful of distinct buffers: the static pool and the current skin slot.
        let mut inputs: Vec<Buffer> = Vec::new();
        for buffer in pending.iter().flat_map(|b| b.inputs) {
            if !inputs.contains(&buffer) {
                inputs.push(buffer);
            }
        }
        recorder.barriers(
            inputs
                .into_iter()
                .map(|b| edge(Resource::Buffer(b), Access::ShaderRead, Access::AsInput)),
        );
        for blas in &pending {
            recorder.push(Command::BuildAccelerationStructure(blas.structure));
        }
        recorder.barriers(pending.iter().flat_map(|b| {
            [Access::AsInput, Access::AsRead].map(|after| {
                edge(
                    Resource::AccelerationStructure(b.structure),
                    Access::AsBuild,
                    after,
                )
            })
        }));
        if let Some(tlas) = self.tlas.as_ref().filter(|t| !t.built) {
            recorder.push(Command::BuildAccelerationStructure(tlas.structure));
        }
    }

    /// Records the final states of a submitted frame: its builds have run.
    pub(super) fn finish(&mut self, final_states: &[State]) {
        let Some(ids) = self.ids.take() else {
            return;
        };
        if let Some(images) = &mut self.images {
            for (state, id) in images.states.iter_mut().zip(ids.images) {
                *state = final_states[id.0];
            }
        }
        if let Some(tlas) = &mut self.tlas {
            tlas.built = true;
        }
        for blas in self.blas.iter_mut().flatten() {
            blas.built = true;
        }
    }
}
