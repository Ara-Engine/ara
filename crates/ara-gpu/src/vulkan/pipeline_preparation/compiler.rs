use super::*;

/// Owns unpublished native objects until adoption or cancellation.
pub(super) struct Batch {
    device: ash::Device,
    pub values: Vec<(pipeline::NativePipeline, PipelinePreparation)>,
}
impl Batch {
    pub fn dispose(mut self: Box<Self>, disposal: &mut disposal::Disposal) {
        for (native, _) in self.values.drain(..) {
            disposal.push(disposal::Object::Pipeline(native.handle));
        }
    }
}
impl Drop for Batch {
    fn drop(&mut self) {
        for (native, _) in self.values.drain(..) {
            // SAFETY: The batch owns only unpublished PSOs, on a retained device.
            unsafe { self.device.destroy_pipeline(native.handle, None) };
        }
    }
}

/// The cache is confined to the compiler thread, including persistence and Drop.
pub(super) struct Compiler {
    device: ash::Device,
    mesh: bool,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    path: PathBuf,
}
impl Compiler {
    pub fn new(
        device: ash::Device,
        mesh: bool,
        layout: vk::PipelineLayout,
        path: PathBuf,
        seed: Vec<u8>,
    ) -> Result<Self> {
        // SAFETY: The seed was checked against this device. No other thread
        // accesses this private cache, including during save and destruction.
        let cache = unsafe {
            device
                .create_pipeline_cache(
                    &vk::PipelineCacheCreateInfo::default().initial_data(&seed),
                    None,
                )
                .or_else(|_| {
                    device.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)
                })
        }
        .map_err(anyhow::Error::from)?;
        Ok(Self {
            device,
            mesh,
            layout,
            cache,
            path,
        })
    }
    pub fn compile(&self, recipes: Vec<PipelinePreparation>) -> Result<Box<Batch>> {
        crate::cpu_scope!("GPU.pipeline_compile_worker");
        let mut batch = Box::new(Batch {
            device: self.device.clone(),
            values: Vec::new(),
        });
        for recipe in recipes {
            let native = pipeline::create_on_device(
                &self.device,
                self.mesh,
                self.layout,
                self.cache,
                &recipe.cooked,
                &recipe.artifacts,
            )?;
            batch.values.push((native, recipe));
        }
        // Persist after successful batches as well as shutdown, so a later
        // process can reuse the PSOs even if this process has a long lifetime.
        self.save();
        Ok(batch)
    }
    fn save(&self) {
        // SAFETY: Only this worker accesses its cache and the device is live.
        if let Ok(bytes) = unsafe { self.device.get_pipeline_cache_data(self.cache) } {
            let _ = cache::persist(&self.path, &bytes);
        }
    }
}
impl Drop for Compiler {
    fn drop(&mut self) {
        self.save();
        // SAFETY: No compile remains active on this worker's private cache.
        unsafe { self.device.destroy_pipeline_cache(self.cache, None) };
    }
}
