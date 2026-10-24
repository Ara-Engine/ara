//! Explicit readbacks of the selected live presentation.
use super::*;

impl Presenter {
    /// Captures the same fidelity presentation used by windows and embedded Game.
    /// Resolves content only when required, with stable GPU-derived entity marks.
    pub fn capture_with(
        &mut self,
        frame: &RenderFrame,
        request: &CaptureRequest,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<Capture> {
        let original_size = self.size;
        let result = self.capture_at(frame, request, resolve);
        self.size = original_size;
        result
    }

    fn capture_at(
        &mut self,
        frame: &RenderFrame,
        request: &CaptureRequest,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<Capture> {
        self.resize(request.size)?;
        self.prepare_with(frame, resolve)?;
        let prepared_stats = self.stats;
        if self.live.is_none() || !matches!(request.mode, ViewMode::Lit) {
            return self.capture_baseline(
                frame,
                request.size,
                request.mode,
                request.marks,
                request.seed,
            );
        }
        if request.deterministic {
            let live = self.live.as_mut().context("native capture view")?;
            let gpu = self
                .renderer
                .as_mut()
                .context("presentation device")?
                .gpu_mut();
            // Captures settle residency before accumulating AA; progressive live
            // refinement must not change the material midway through a readback.
            let mut settled = false;
            for _ in 0..256 {
                let stats = live.view.refine_native_textures(gpu)?;
                if stats.pending_textures == 0 {
                    settled = true;
                    break;
                }
                ensure!(
                    stats.uploaded_bytes > 0,
                    "capture texture residency made no progress"
                );
            }
            ensure!(settled, "capture texture residency did not settle");
            live.effects.reset(gpu)?;
            live.view.reset_accumulation(gpu)?;
            live.tick = None;
            live.temporal = request.seed as u32;
            live.lighting_changed = false;
        }
        for _ in 0..request.settle_frames.max(1) {
            self.present(frame)?;
        }
        self.stats.rebuilt |= prepared_stats.rebuilt;
        self.stats.updated = self.stats.updated.max(prepared_stats.updated);
        ensure!(
            self.failure.is_none(),
            "fidelity capture failed: {:?}",
            self.failure
        );
        let live = self.live.as_mut().context("native capture view")?;
        let gpu = self
            .renderer
            .as_mut()
            .context("presentation device")?
            .gpu_mut();
        let image = live.view.capture(gpu)?;
        let low = live.view.capture_entity_ids(gpu)?;
        let size = live.plan.render_size;
        let mut ids = Vec::with_capacity(image.width() as usize * image.height() as usize);
        for y in 0..image.height() {
            for x in 0..image.width() {
                ids.push(
                    low[(u64::from(y) * u64::from(size[1]) / u64::from(image.height())) as usize
                        * size[0] as usize
                        + (u64::from(x) * u64::from(size[0]) / u64::from(image.width())) as usize],
                );
            }
        }
        let names: Vec<_> = live
            .prepared
            .scene
            .instances
            .iter()
            .map(|i| (i.entity, i.name.to_owned()))
            .collect();
        let root = ara_gpu::cooked::resource_dir(
            "ara-render-fidelity",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-render-fidelity/cooked"),
        );
        let manifest: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(root.join("reference.json"))?)?;
        let shaders = ShaderMetadata {
            content_key: manifest.key,
            compiler: manifest.compiler,
            binding_layout_hash: manifest.binding_layout_hash,
        };
        Capture::from_feature(
            FeatureCapture {
                rgba: image.into_raw(),
                ids,
                names: &names,
                hash: live.view.scene_hash(),
                device: &gpu.context().info,
                timings: live.view.timings(),
                shaders: &shaders,
            },
            request,
        )
    }

    /// Lit captures use fidelity by default on RT hardware.
    pub fn capture(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        mode: ViewMode,
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        self.capture_with(
            frame,
            &CaptureRequest {
                camera: camera(frame)?,
                size,
                mode,
                deterministic: true,
                exposure: frame.environment.exposure,
                settle_frames: 16,
                tick: frame.tick,
                seed,
                marks,
            },
            || Ok(Content::default()),
        )
    }

    /// Captures the selected window/Game presentation, including fidelity terrain and VFX.
    pub fn capture_presented(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        self.prepare(frame)?;
        if self.live.is_none() {
            return self.capture_presented_baseline(frame, size, marks, seed);
        }
        self.capture(frame, size, ViewMode::Lit, marks, seed)
    }
}
