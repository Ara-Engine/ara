mod capture;
mod record;
use crate::*;
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Device, GpuDevice, Pipeline, Queue, Resource, RootConstants, TimelinePoint,
    commands::{Command, CommandList},
    cooked::CookedPipeline,
    vk,
};
use ara_render::graph::{Graph, State};
use std::path::Path;
const PACKET_WORDS: usize = 4 + (TILE_SIZE * TILE_SIZE) as usize;
/// Persistent GPU virtual texture cache. Host owns the submission/completion
/// boundary; prepare decodes pages, begin/end record without waiting or I/O.
pub struct Residency {
    catalog: Catalog,
    pool: Pool,
    buffers: [Buffer; 7],
    slots: [u32; 6],
    pipelines: [Pipeline; 3],
    previous: Vec<State>,
    epoch: u32,
    pending: u32,
    upload_pages: usize,
    size: [u32; 2],
    last: Option<TimelinePoint>,
}
impl Residency {
    /// Creates a byte-budgeted physical pool and diagnostic surface. The source
    /// catalog is owned; cooks must already exist at `cooked` (offline only).
    pub fn new(
        gpu: &mut GpuDevice,
        catalog: Catalog,
        budget: u64,
        upload_pages: usize,
        size: [u32; 2],
        cooked: &Path,
    ) -> Result<Self> {
        crate::validate_device(gpu)?;
        ensure!(
            !size.contains(&0)
                && size.iter().all(|v| *v <= 8192)
                && catalog.sources.len() <= 32
                && upload_pages <= 1024,
            "invalid texture capacities"
        );
        let pool = Pool::new(&catalog, budget, upload_pages)?;
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let transfer = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
        let desc = upload(gpu, bytemuck::cast_slice(&catalog.descriptors))?;
        let table = upload(gpu, &vec![0; catalog.page_count() * 4])?;
        let physical = gpu.create_buffer(pool.bytes(), storage | transfer, false)?;
        let feedback = upload(gpu, &vec![0; (catalog.page_count() + 128) * 4])?;
        let stage = gpu.create_buffer((upload_pages * PACKET_WORDS * 4) as u64, storage, true)?;
        let diagnostics = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 16,
            storage | transfer,
            false,
        )?;
        let read = gpu.create_buffer(
            (catalog.page_count() + 128) as u64 * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let buffers = [desc, table, physical, feedback, stage, diagnostics, read];
        let mut slots = [0; 6];
        for (slot, &buffer) in slots.iter_mut().zip(&buffers) {
            *slot = gpu.storage_buffer(buffer)?.index;
        }
        let mut pipelines = Vec::new();
        for name in ["texture_upload", "texture_publish", "texture_reset"] {
            let manifest: CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            pipelines.push(gpu.create_pipeline(&manifest, cooked)?);
        }
        let mut result = Self {
            catalog,
            pool,
            buffers,
            slots,
            pipelines: pipelines
                .try_into()
                .map_err(|_| anyhow::anyhow!("texture pipelines"))?,
            previous: Vec::new(),
            epoch: 0,
            pending: 0,
            upload_pages,
            size,
            last: None,
        };
        // Initialize every mip tail before publishing a binding to consumers.
        // Multiple bounded batches are preparation-only synchronous work.
        let pinned = result.catalog.pages.iter().filter(|p| p.pinned).count() as u32;
        while result.pool.resident() < pinned {
            result.prepare(gpu)?;
            let point = gpu.submit(result.begin_frame(gpu)?, &[])?;
            gpu.resolve(point)?;
            let point = gpu.submit(result.end_frame(gpu)?, &[])?;
            gpu.resolve(point)?;
            result.submitted(point);
        }
        Ok(result)
    }
    /// Consumes completed GPU feedback and prepares bounded decoded uploads.
    /// May run on an extraction worker; never call before prior submission completes.
    pub fn prepare(&mut self, gpu: &GpuDevice) -> Result<()> {
        if let Some(point) = self.last {
            ensure!(
                gpu.completed()?[point.queue.index()] >= point.value,
                "texture frame in flight"
            );
        }
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("texture feedback epoch exhausted"))?;
        let feedback = if self.last.is_some() {
            gpu.read(self.buffers[6])?
                .as_chunks::<4>()
                .0
                .iter()
                .skip(128)
                .map(|b| u32::from_le_bytes(*b))
                .collect()
        } else {
            vec![0; self.catalog.page_count()]
        };
        let uploads = self.pool.request(&self.catalog, self.epoch, &feedback)?;
        let mut packets = Vec::with_capacity(uploads.len() * PACKET_WORDS);
        for upload in &uploads {
            packets.extend([
                upload.page,
                upload.slot,
                upload.evicted.unwrap_or(u32::MAX),
                0,
            ]);
            packets.extend(
                self.catalog
                    .page(upload.page)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b)),
            );
        }
        if !packets.is_empty() {
            gpu.upload(self.buffers[4], bytemuck::cast_slice(&packets))?;
        }
        self.pending = uploads.len() as u32;
        Ok(())
    }
    /// Matching immutable descriptor slots and reachability for this frame.
    pub fn binding(&self) -> Binding {
        Binding {
            slots: [self.slots[0], self.slots[1], self.slots[2], self.slots[3]],
            tracking: [self.epoch, self.slots[5], self.size[0], self.size[1]],
            resources: [
                self.buffers[0],
                self.buffers[1],
                self.buffers[2],
                self.buffers[3],
                self.buffers[5],
            ],
            textures: self.catalog.sources.len() as u32,
        }
    }
    /// Record the submitted end-of-frame timeline before preparing another frame.
    pub fn submitted(&mut self, point: TimelinePoint) {
        self.last = Some(point);
    }
    /// Records uploads, publication and counter reset. Sampling follows this list.
    pub fn begin_frame(&mut self, gpu: &GpuDevice) -> Result<CommandList> {
        self.record(gpu, false)
    }
    /// Records feedback readback after every sampling pass has completed on Graphics.
    pub fn end_frame(&mut self, gpu: &GpuDevice) -> Result<CommandList> {
        self.record(gpu, true)
    }
}
fn upload(gpu: &mut GpuDevice, bytes: &[u8]) -> Result<Buffer> {
    let buffer = gpu.create_buffer(
        bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::TRANSFER_SRC,
        false,
    )?;
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, bytes)?;
    let mut r = gpu.begin(Queue::Graphics);
    r.push(Command::CopyBuffer {
        source: stage,
        destination: buffer,
        size: bytes.len() as u64,
    });
    r.barriers([ara_gpu::Barrier {
        resource: Resource::Buffer(buffer),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let p = gpu.submit(r.finish(), &[])?;
    gpu.resolve(p)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    Ok(buffer)
}
