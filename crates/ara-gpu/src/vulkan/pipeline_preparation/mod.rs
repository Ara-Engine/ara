//! Bounded PSO compilation with owner-thread adoption and a private persisted cache.
use super::*;
use crate::arena::Handle;
use std::path::PathBuf;
mod compiler;
mod worker;
pub(super) use worker::Worker;

/// One offline-cooked pipeline to compile without touching frame queues or heaps.
pub struct PipelinePreparation {
    /// Versioned, hashed shader and render-state description.
    pub cooked: cooked::CookedPipeline,
    /// Directory containing this description's checked SPIR-V artifacts.
    pub artifacts: PathBuf,
}

/// Type tag for a pipeline compilation request owned by one device.
pub enum PipelinePreparationKind {}
/// An unpublished pipeline batch. Poll to adopt it, or explicitly cancel it.
pub type PipelinePreparationId = Handle<PipelinePreparationKind>;

impl VulkanDevice {
    /// Admit at most two pipeline batches, including completed but unadopted
    /// batches. The factory runs only when admitted. Compilation uses one worker
    /// and its own driver cache; it never submits work or locks the frame cache.
    pub fn prepare_pipelines(
        &mut self,
        build: impl FnOnce() -> Vec<PipelinePreparation>,
    ) -> Result<Option<PipelinePreparationId>> {
        if self.pipeline_preparation.is_none() {
            self.pipeline_preparation = Some(Worker::new(
                self.context.device.clone(),
                self.context.info.mesh_shading,
                self.heap.layout,
                self.cache_path.with_extension("async.bin"),
                &self.context.info,
            )?);
        }
        self.pipeline_preparation
            .as_mut()
            .expect("pipeline worker initialized")
            .submit(
                self.disposal.as_mut().expect("disposal worker alive"),
                build,
            )
    }

    /// Poll without waiting or creating a native pipeline on this thread. Ready
    /// handles have the same lifetime/retirement rules as synchronous pipelines.
    pub fn poll_prepared_pipelines(
        &mut self,
        id: PipelinePreparationId,
    ) -> Result<Option<Vec<Pipeline>>> {
        let result = self
            .pipeline_preparation
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .poll(self.disposal.as_mut().expect("disposal worker alive"), id)?;
        let Some(batch) = result else { return Ok(None) };
        let mut batch = batch?;
        let mut handles = Vec::with_capacity(batch.values.len());
        while let Some((native, source)) = batch.values.pop() {
            let handle = self.pipelines.insert(native)?;
            if let Some(worker) = &mut self.shader_worker
                && let Err(error) = worker.track(handle, &source.cooked, &source.artifacts)
            {
                handles.push(handle);
                for handle in handles {
                    self.retire_pipeline(handle)?;
                }
                return Err(error.into());
            }
            handles.push(handle);
        }
        handles.reverse();
        Ok(Some(handles))
    }

    /// Cancel without joining the compiler. Its unpublished PSOs are disposed
    /// after compilation, without entering a resource arena or frame submission.
    pub fn cancel_prepared_pipelines(&mut self, id: PipelinePreparationId) -> Result<()> {
        self.pipeline_preparation
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .cancel(self.disposal.as_mut().expect("disposal worker alive"), id)
    }
}
