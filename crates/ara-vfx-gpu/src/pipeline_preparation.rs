//! Live effects skip drawing until their cooked PSOs finish off-thread.
use anyhow::{Result, ensure};
use ara_gpu::{
    GpuDevice, Pipeline,
    cooked::CookedPipeline,
    vulkan::{PipelinePreparation, PipelinePreparationId},
};
use std::{collections::VecDeque, path::Path};

/// A bounded compilation request for one cooked effect. Poll at frame boundaries
/// or cancel explicitly when the effect disappears; it owns no simulation state.
#[must_use = "poll or cancel the pipeline warmup"]
pub struct PipelineWarmup {
    recipes: Vec<PipelinePreparation>,
    ticket: Option<PipelinePreparationId>,
    key: String,
}

/// Ready PSOs tied to one checked effect cohort. Consume in
/// `GpuSystem::new_prepared`, or retire without allocating an effect system.
#[must_use = "construct a system or retire the prepared pipelines"]
pub struct PreparedPipelines {
    pub(crate) handles: VecDeque<Pipeline>,
    pub(crate) key: String,
}
impl PreparedPipelines {
    /// Retire an unused batch without changing a live effect.
    pub fn retire(self, gpu: &mut GpuDevice) -> Result<()> {
        for pipeline in self.handles {
            gpu.retire_pipeline(pipeline)?;
        }
        Ok(())
    }
}
impl PipelineWarmup {
    /// Read the immutable cook once. No GPU or shader compiler is needed here.
    pub fn new(cooked: &Path) -> Result<Self> {
        let mut recipes = Vec::new();
        let mut key = None;
        for name in
            crate::COMPUTE_ENTRIES
                .iter()
                .copied()
                .chain(["backdrop", "particles", "tonemap"])
        {
            let source: CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            if let Some(key) = &key {
                ensure!(*key == source.key, "mixed VFX shader cohort");
            } else {
                key = Some(source.key.to_owned());
            }
            recipes.push(PipelinePreparation {
                cooked: source,
                artifacts: cooked.to_owned(),
            });
        }
        Ok(Self {
            recipes,
            ticket: None,
            key: key.expect("VFX entries are nonempty"),
        })
    }
    /// Submit when the bounded worker has room, then poll without waiting. While
    /// None, the host keeps the current scene image and omits this effect.
    pub fn poll(&mut self, gpu: &mut GpuDevice) -> Result<Option<PreparedPipelines>> {
        if self.ticket.is_none() {
            self.ticket = gpu.prepare_pipelines(|| std::mem::take(&mut self.recipes))?;
        }
        let Some(ticket) = self.ticket else {
            return Ok(None);
        };
        let result = gpu.poll_prepared_pipelines(ticket);
        if result.is_err() {
            self.ticket = None;
        }
        let Some(handles) = result? else {
            return Ok(None);
        };
        self.ticket = None;
        Ok(Some(PreparedPipelines {
            handles: handles.into(),
            key: std::mem::take(&mut self.key),
        }))
    }
    /// Cancel a request, including one compiling now, without a host join.
    pub fn cancel(self, gpu: &mut GpuDevice) -> Result<()> {
        if let Some(ticket) = self.ticket {
            gpu.cancel_prepared_pipelines(ticket)?;
        }
        Ok(())
    }
}
