//! Off-thread resource allocation/initial host copies and pipeline compilation,
//! adopted on the device owner. Same request and result types as the Vulkan
//! backend; the workers never submit commands or touch the owner's tables.
use super::MetalDevice;
use super::native::pool;
use super::pipeline::{self, NativePipeline};
use super::resources::{self, NativeBuffer, NativeImage};
use super::worker::Worker;
use crate::vulkan::{
    PipelinePreparation, PipelinePreparationId, PipelinePreparationKind, PreparationId,
    PreparationKind, PreparedResources, ResourcePreparation,
};
use crate::{GpuError, Pipeline, Result};
use objc2::runtime::ProtocolObject;
use objc2_metal::MTLDevice;

/// Freshly created, unpublished resources moving from the worker to the owner.
pub(super) struct Prepared {
    buffers: Vec<NativeBuffer>,
    images: Vec<NativeImage>,
}
// SAFETY: Metal resource objects may be retained, released and used from any thread
// (Metal's threading model); objc2 only withholds `Send` because of their raw
// `contents()` access. The worker keeps no reference after sending, so the owner is
// the only thread that touches them afterwards.
unsafe impl Send for Prepared {}

/// The resource job: allocate every buffer and image and copy initial host data.
fn allocate(
    device: &ProtocolObject<dyn MTLDevice>,
    request: ResourcePreparation,
) -> Result<Prepared> {
    let mut buffers = Vec::with_capacity(request.buffers.len());
    for buffer in request.buffers {
        if !buffer.data.is_empty() && !buffer.host {
            return Err(GpuError::Contract(
                "Initial data requires a host-visible buffer".into(),
            ));
        }
        let raw = resources::buffer(device, buffer.size, buffer.usage, buffer.host)?;
        let data = buffer.data.load()?;
        if !data.is_empty() {
            resources::write(&raw, 0, &data)?;
        }
        buffers.push(raw);
    }
    let images = request
        .images
        .into_iter()
        .map(|i| resources::image(device, i.size, i.format, i.mip_levels, i.usage))
        .collect::<Result<_>>()?;
    Ok(Prepared { buffers, images })
}

pub(super) type ResourceWorker = Worker<PreparationKind, ResourcePreparation, Prepared>;
pub(super) type PipelineWorker = Worker<
    PipelinePreparationKind,
    Vec<PipelinePreparation>,
    Vec<(NativePipeline, PipelinePreparation)>,
>;

impl MetalDevice {
    /// Queue allocation and initial host copies off-thread. At most two requests are
    /// retained; `build` runs only when admitted, and None keeps the caller's input.
    pub fn prepare_resources(
        &mut self,
        build: impl FnOnce() -> ResourcePreparation,
    ) -> Result<Option<PreparationId>> {
        if self.resource_worker.is_none() {
            let device = self.device.clone();
            self.resource_worker = Some(Worker::new(
                "ara-metal-resources",
                move |request: ResourcePreparation| pool(|| allocate(&device, request)),
            )?);
        }
        self.resource_worker
            .as_mut()
            .expect("resource worker initialized")
            .submit(build)
    }
    /// Poll without waiting. A completed request moves sole ownership into this
    /// device and expires its ticket; a failed request adopts nothing.
    pub fn poll_prepared_resources(
        &mut self,
        id: PreparationId,
    ) -> Result<Option<PreparedResources>> {
        let worker = self.resource_worker.as_mut().ok_or(GpuError::StaleHandle)?;
        let Some(result) = worker.poll(id)? else {
            return Ok(None);
        };
        let prepared = result?;
        let buffers = prepared
            .buffers
            .into_iter()
            .map(|raw| self.adopt_buffer(raw))
            .collect::<Result<_>>()?;
        let images = prepared
            .images
            .into_iter()
            .map(|raw| self.adopt_image(raw))
            .collect::<Result<_>>()?;
        Ok(Some(PreparedResources { buffers, images }))
    }
    /// Cancel without waiting; a running allocation finishes and is dropped unseen.
    pub fn cancel_prepared_resources(&mut self, id: PreparationId) -> Result<()> {
        self.resource_worker
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .cancel(id)
    }
    /// Compile a batch of cooked pipelines off-thread (at most two batches retained).
    pub fn prepare_pipelines(
        &mut self,
        build: impl FnOnce() -> Vec<PipelinePreparation>,
    ) -> Result<Option<PipelinePreparationId>> {
        if self.pipeline_worker.is_none() {
            let (device, compiler) = (self.device.clone(), self.compiler.clone());
            self.pipeline_worker = Some(Worker::new(
                "ara-metal-pipelines",
                move |batch: Vec<PipelinePreparation>| {
                    pool(|| {
                        batch
                            .into_iter()
                            .map(|p| {
                                pipeline::create(&device, &compiler, &p.cooked, &p.artifacts)
                                    .map(|native| (native, p))
                            })
                            .collect()
                    })
                },
            )?);
        }
        self.pipeline_worker
            .as_mut()
            .expect("pipeline worker initialized")
            .submit(build)
    }
    /// Poll without waiting; a finished batch is adopted in request order.
    pub fn poll_prepared_pipelines(
        &mut self,
        id: PipelinePreparationId,
    ) -> Result<Option<Vec<Pipeline>>> {
        let worker = self.pipeline_worker.as_mut().ok_or(GpuError::StaleHandle)?;
        let Some(result) = worker.poll(id)? else {
            return Ok(None);
        };
        let mut handles = Vec::new();
        for (native, source) in result? {
            let handle = self.pipelines.insert(native)?;
            handles.push(handle);
            if let Some(worker) = &mut self.shader_worker
                && let Err(error) = worker.track(handle, &source.cooked, &source.artifacts)
            {
                for handle in handles {
                    self.retire_pipeline(handle)?;
                }
                return Err(error.into());
            }
        }
        Ok(Some(handles))
    }
    /// Cancel without waiting; a running batch finishes and is dropped unseen.
    pub fn cancel_prepared_pipelines(&mut self, id: PipelinePreparationId) -> Result<()> {
        self.pipeline_worker
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .cancel(id)
    }
}
