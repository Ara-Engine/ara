//! `Command::ClearDepth` on Metal. Metal has no in-pass rectangle clear, so the cooked
//! vertex-only `clear_depth` pipeline (`shaders/clear_depth.slang`) draws a fullscreen
//! triangle at the clear depth; the encoder clips it to the tile with viewport and
//! scissor, tests Always and writes depth. Render pipelines must match the scope's
//! color formats, so variants are built per format set (color writes masked off).
use super::cache::Compiler;
use super::native::nil;
use super::pipeline;
use crate::cooked::{self, CookedPipeline};
use crate::{GpuError, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;

type RenderState = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// The clear function and its pipeline variants.
pub(super) struct Clear {
    library: Retained<ProtocolObject<dyn MTLLibrary>>,
    entry: String,
    pub depth: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    variants: Vec<(Vec<MTLPixelFormat>, RenderState)>,
}

impl Clear {
    /// Load the cooked clear pipeline shipped with `ara-gpu`.
    pub fn cooked(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        let dir = cooked::resource_dir("ara-gpu", concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"));
        let manifest: CookedPipeline = serde_json::from_slice(
            &std::fs::read(dir.join("clear_depth.json")).map_err(anyhow::Error::from)?,
        )
        .map_err(anyhow::Error::from)?;
        let stage = manifest
            .stages
            .first()
            .ok_or_else(|| GpuError::Contract("clear_depth has no stage".into()))?;
        let (_, library, reflection) = pipeline::load(device, &manifest, stage, &dir)?;
        Self::new(device, library, reflection.entry_point)
    }
    /// A clear around an already loaded vertex function (cooked, or tests).
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        library: Retained<ProtocolObject<dyn MTLLibrary>>,
        entry: String,
    ) -> Result<Self> {
        let stencil = MTLDepthStencilDescriptor::new();
        stencil.setDepthCompareFunction(MTLCompareFunction::Always);
        stencil.setDepthWriteEnabled(true);
        let depth = device
            .newDepthStencilStateWithDescriptor(&stencil)
            .ok_or_else(|| nil("clear depth state"))?;
        Ok(Self {
            library,
            entry,
            depth,
            variants: Vec::new(),
        })
    }
    /// The variant for a scope with these color formats, built on first use.
    pub fn pipeline(
        &mut self,
        compiler: &Compiler,
        colors: &[MTLPixelFormat],
    ) -> Result<RenderState> {
        if let Some((_, state)) = self.variants.iter().find(|(formats, _)| formats == colors) {
            return Ok(state.clone());
        }
        let state = pipeline::depth_only(compiler, &self.library, &self.entry, colors)?;
        self.variants.push((colors.to_vec(), state.clone()));
        Ok(state)
    }
}
