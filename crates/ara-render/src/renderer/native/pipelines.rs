//! Three native PSOs are retained by the renderer across content generations.
use super::*;
use ara_gpu::vulkan::{PipelinePreparation, PipelinePreparationId};

#[derive(Default)]
pub(in crate::renderer) enum Pipelines {
    #[default]
    Empty,
    Pending(PipelinePreparationId),
    Ready([Pipeline; 3]),
}
impl Renderer {
    /// Poll the native fill/shadow/forward PSOs without compiling or waiting on
    /// this thread. Content stays unpublished until this and its uploads are ready.
    pub fn preallocate_native_pipelines(&mut self) -> Result<bool> {
        if matches!(self.native_pipelines, Pipelines::Empty) {
            let mut recipes = Vec::new();
            for name in ["native_shadow", "native_gbuffer", "native_forward"] {
                let cooked: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(
                    &std::fs::read(self.artifacts.join(format!("{name}.json")))?,
                )?;
                ensure!(
                    cooked.key == self.shader_metadata.content_key,
                    "native cook is incomplete"
                );
                recipes.push(PipelinePreparation {
                    cooked,
                    artifacts: self.artifacts.to_owned(),
                });
            }
            if let Some(ticket) = self.gpu.prepare_pipelines(|| recipes)? {
                self.native_pipelines = Pipelines::Pending(ticket);
            }
        }
        if let Pipelines::Pending(ticket) = self.native_pipelines {
            let result = self.gpu.poll_prepared_pipelines(ticket);
            if result.is_err() {
                self.native_pipelines = Pipelines::Empty;
            }
            if let Some(handles) = result? {
                self.native_pipelines = Pipelines::Ready(
                    handles
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("native PSO count differs"))?,
                );
            }
        }
        Ok(matches!(self.native_pipelines, Pipelines::Ready(_)))
    }
    pub(super) fn native_pipeline_handles(&mut self) -> Result<[Pipeline; 3]> {
        while !self.preallocate_native_pipelines()? {
            ensure!(
                !self.gpu.deferring_completion(),
                "preallocate native PSOs before asynchronous publication"
            );
            // Only explicit synchronous constructors/captures wait. Interactive
            // content calls preallocate and retains its last completed image.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let Pipelines::Ready(handles) = self.native_pipelines else {
            unreachable!("PSOs ready")
        };
        Ok(handles)
    }
}
