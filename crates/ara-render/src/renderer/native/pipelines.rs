//! Twelve native PSOs are retained by the renderer across content generations.
use super::*;
use ara_gpu::vulkan::{PipelinePreparation, PipelinePreparationId};

/// The native PSOs (`shaders/pipelines.json`), prepared together behind loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::renderer) struct NativePipelines {
    /// Shadow casters: cascades, the far map, local faces and VSM pages.
    pub shadow: Pipeline,
    /// The G-buffer fill, terrain with steep-slope side projections.
    pub gbuffer: Pipeline,
    /// The forward pass (blended and transmissive materials) of the debug views,
    /// which read the G-buffer it blends into.
    pub forward: Pipeline,
    /// The forward pass of lit frames: colour, ids and motion only, since nothing
    /// reads the G-buffer after it. Front faces only: `forward_lit_back` draws an
    /// instance's back faces first, so closed films composite far side under near.
    pub forward_lit: Pipeline,
    /// The G-buffer fill for presets without terrain detail
    /// (`Quality::terrain_detail` zero): terrain keeps its top projection, and
    /// compiled without the side projections, every pixel saves their registers.
    pub gbuffer_top: Pipeline,
    /// Triangle edges for the wireframe view modes (`ViewMode::wire`): the camera
    /// draws again into the wire mask, depth-tested against the frame's depth.
    pub wire: Pipeline,
    /// The forward pass with the High/Epic tier's code (`Quality::high_tier`).
    pub forward_high: Pipeline,
    /// Transmissive forward draws (`batches::is_refractive`): refraction of the
    /// copied opaque scene, absorption and Fresnel, composited as final colour
    /// (look-refract.slangh), so ordinary transparency compiles none of it.
    pub refract: Pipeline,
    /// The refractive forward pass with the High/Epic tier's code.
    pub refract_high: Pipeline,
    /// The back faces of lit frames' blended draws, before `forward_lit`.
    pub forward_lit_back: Pipeline,
    /// The back faces of blended draws with the High/Epic tier's code.
    pub forward_high_back: Pipeline,
    /// Blended casters' film opacity into the translucent sun map (`look::translucent`).
    pub translucent: Pipeline,
}

impl NativePipelines {
    /// The G-buffer PSO for a preset's `Quality::terrain_detail`: without terrain
    /// detail, the one compiled without side projections.
    pub fn gbuffer_for(&self, terrain_detail: f32) -> Pipeline {
        if terrain_detail > 0. {
            self.gbuffer
        } else {
            self.gbuffer_top
        }
    }
}

/// Cooked recipe names, in `NativePipelines` field order.
const RECIPES: [&str; 12] = [
    "native_shadow",
    "native_gbuffer",
    "native_forward",
    "native_forward_lit",
    "native_gbuffer_top",
    "native_wire",
    "native_forward_high",
    "native_forward_refract",
    "native_forward_refract_high",
    "native_forward_lit_back",
    "native_forward_high_back",
    "native_shadow_translucent",
];

#[derive(Default)]
pub(in crate::renderer) enum Pipelines {
    #[default]
    Empty,
    Pending(PipelinePreparationId),
    Ready(Box<NativePipelines>),
}
impl Renderer {
    /// Poll the native PSOs without compiling or waiting on this thread. Content
    /// stays unpublished until this and its uploads are ready.
    pub fn preallocate_native_pipelines(&mut self) -> Result<bool> {
        if matches!(self.native_pipelines, Pipelines::Empty) {
            let mut recipes = Vec::new();
            for name in RECIPES {
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
                let [
                    shadow,
                    gbuffer,
                    forward,
                    forward_lit,
                    gbuffer_top,
                    wire,
                    forward_high,
                    refract,
                    refract_high,
                    forward_lit_back,
                    forward_high_back,
                    translucent,
                ]: [Pipeline; RECIPES.len()] = handles
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("native PSO count differs"))?;
                self.native_pipelines = Pipelines::Ready(Box::new(NativePipelines {
                    shadow,
                    gbuffer,
                    forward,
                    forward_lit,
                    gbuffer_top,
                    wire,
                    forward_high,
                    refract,
                    refract_high,
                    forward_lit_back,
                    forward_high_back,
                    translucent,
                }));
            }
        }
        Ok(matches!(self.native_pipelines, Pipelines::Ready(_)))
    }
    pub(super) fn native_pipeline_handles(&mut self) -> Result<NativePipelines> {
        while !self.preallocate_native_pipelines()? {
            ensure!(
                !self.gpu.deferring_completion(),
                "preallocate native PSOs before asynchronous publication"
            );
            // Only explicit synchronous constructors/captures wait. Interactive
            // content calls preallocate and retains its last completed image.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let Pipelines::Ready(handles) = &self.native_pipelines else {
            unreachable!("PSOs ready")
        };
        Ok(**handles)
    }
}
