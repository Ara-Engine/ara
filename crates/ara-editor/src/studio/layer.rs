//! A studio native layer drawn by its own GPU UI adapter on the renderer's device: the
//! Game view and the graph canvases paint `ara_ui` display lists into one each. Submissions
//! never wait (`ready` skips a pass while the last frame is still on the GPU).
use anyhow::Result;
use ara_ui::paint::DisplayList;
use ara_ui_render::atlas::TextMode;
use ara_ui_render::gpu::Renderer as UiRenderer;
use ara_ui_text::FontAsset;

use crate::gpu::{NativeUi, ui_device};

/// One layer's adapter, paint renderer and presentation.
pub(super) struct Layer {
    pub ui: NativeUi,
    paint: UiRenderer,
    /// The presentation, its size and the layer it presents into.
    present: Option<(ara_platform::Present, [u32; 2], usize)>,
}

impl Layer {
    /// An adapter on the renderer's GPU that draws external images `images` (the paint
    /// renderer skips images it was not told about).
    /// # Safety
    /// The renderer outlives this; one thread submits for both.
    pub unsafe fn new(
        renderer: &ara_render::Renderer,
        images: impl IntoIterator<Item = u32>,
    ) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        let mut ui = unsafe { NativeUi::new(ui_device(renderer.gpu()))? };
        ui.set_deferred_completion(true)?;
        let mut paint = UiRenderer::default();
        for id in images {
            paint
                .external_image(id)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(Self {
            ui,
            paint,
            present: None,
        })
    }

    /// Uploads image `id` (an in-node preview), drawn wherever the list names it.
    pub fn image(&mut self, id: u32, image: ara_ui_render::gpu::Image) -> Result<()> {
        self.paint
            .image(id, image)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Whether the last frame left the GPU (else skip this pass and draw on a later one).
    pub fn ready(&mut self, renderer: &ara_render::Renderer) -> Result<bool> {
        #[cfg(any(windows, target_os = "linux"))]
        let started = std::time::Instant::now();
        let budget = std::time::Duration::from_millis(1);
        if !self.ui.wait_previous_for(budget)? {
            return Ok(false);
        }
        // Windows' bounded blit may still read the target the next draw resizes.
        #[cfg(any(windows, target_os = "linux"))]
        {
            if let Some((present, _, _)) = &mut self.present {
                // SAFETY: the layer and renderer retain the presentation's creating device.
                if !unsafe {
                    present.settled(renderer.gpu(), budget.saturating_sub(started.elapsed()))?
                } {
                    return Ok(false);
                }
            }
            self.ui.order_before_scene(ui_device(renderer.gpu()))?;
        }
        let _ = renderer;
        Ok(true)
    }

    /// Draws `list` (shaped with `fonts`) into `layer` at `extent` physical pixels.
    /// # Safety
    /// `layer` is the live native layer this presents into (`present::Target`).
    pub unsafe fn draw(
        &mut self,
        renderer: &ara_render::Renderer,
        layer: super::present::Target,
        extent: [u32; 2],
        scale: f32,
        list: &DisplayList,
        fonts: &[FontAsset],
    ) -> Result<()> {
        self.ui.set_dpi(scale)?;
        let size =
            [extent[0] as f32 / scale, extent[1] as f32 / scale].map(|v| v.round().max(1.0) as u32);
        super::trace::time("layer paint", std::time::Duration::from_millis(3), || {
            self.paint
                .render(&mut self.ui, list, fonts, size, TextMode::Raster)
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let target = layer.id();
        // Another layer (a different graph page) gets its own presentation.
        if let Some((mut old, _, _)) = self.present.take_if(|(_, _, at)| *at != target) {
            let _ = self.ui.wait_idle(ui_device(renderer.gpu()));
            // SAFETY: the adapter is idle and the presentation is not used again.
            unsafe { self.ui.retire_present(ui_device(renderer.gpu()), &mut old) };
        }
        let present = match self.present.take() {
            Some((mut present, was, _)) => {
                if was != extent {
                    super::present::resize(&mut present, extent)?;
                }
                present
            }
            // SAFETY: forwarded caller contract.
            None => unsafe { super::present::create(&self.ui, renderer, layer, extent)? },
        };
        let (present, _, _) = self.present.insert((present, extent, target));
        // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
        super::trace::time("layer present", std::time::Duration::from_millis(3), || {
            // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
            unsafe { self.ui.present(ui_device(renderer.gpu()), present) }
        })?;
        Ok(())
    }

    /// Waits for and releases the presentation; call before the renderer drops.
    pub fn retire(&mut self, renderer: &ara_render::Renderer) {
        let _ = self.ui.wait_idle(ui_device(renderer.gpu()));
        if let Some((mut present, _, _)) = self.present.take() {
            // SAFETY: the adapter is idle and the presentation is not used again.
            unsafe {
                self.ui
                    .retire_present(ui_device(renderer.gpu()), &mut present)
            };
        }
    }
}
