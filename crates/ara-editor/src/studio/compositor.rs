//! The Scene view's GPU compositor: the renderer's image with the editor's scene overlays
//! (grid, selection and agent outlines by entity, the transform gizmo) drawn over it on the
//! GPU UI adapter, then presented into the studio's native layer. The overlays are the same
//! `crate::overlay` geometry and shaders the legacy window used; only the host changed.

use anyhow::Result;
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;
use ara_ui_render::atlas::TextMode;
use ara_ui_render::gpu::Renderer as UiRenderer;

use crate::gpu::{NativeUi, ui_device};
use crate::overlay::{self, GizmoState, Outlines};
use crate::state::EditorState;
use crate::world::DocumentView;

/// Composites and presents the Scene layer. Create it once the renderer is ready; drop it
/// (after [`Self::retire`]) before the renderer.
pub(super) struct Compositor {
    ui: NativeUi,
    paint: UiRenderer,
    outlines: Outlines,
    sheet: ara_ui::style::Sheet,
    light: bool,
    present: Option<(ara_platform::Present, [u32; 2])>,
}

impl Compositor {
    /// An adapter on the renderer's GPU, themed for `light`.
    /// # Safety
    /// The renderer outlives the compositor; one thread submits for both.
    pub unsafe fn new(renderer: &ara_render::Renderer, light: bool) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        let mut ui = unsafe { NativeUi::new(ui_device(renderer.gpu()))? };
        // Submissions return at once; the next frame checks the last one finished (the
        // UI thread never waits on the GPU, as the legacy window does).
        ui.set_deferred_completion(true)?;
        let mut paint = UiRenderer::default();
        paint
            .external_image(overlay::viewport_image())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            ui,
            paint,
            outlines: Outlines::default(),
            sheet: sheet(light)?,
            light,
            present: None,
        })
    }

    /// The map from presented renderer entities to scene addresses, brought up to date
    /// (Drop to Surface reads what the rays hit through it).
    pub fn channels(
        &mut self,
        renderer: &ara_render::Renderer,
        view: &DocumentView,
        revision: u64,
    ) -> &crate::world::channels::Channels {
        self.outlines.channels.update(
            crate::world::EditWorldView::entities(view),
            revision,
            renderer.entities(),
        );
        &self.outlines.channels
    }

    /// The renderer entity the click at physical pixel `at` lands on, through the
    /// channel map (a model's parts and a terrain's tiles pick their entity).
    pub fn pick(
        &mut self,
        renderer: &mut ara_render::Renderer,
        view: &DocumentView,
        revision: u64,
        at: [u32; 2],
    ) -> Result<Option<String>> {
        self.outlines.channels.update(
            crate::world::EditWorldView::entities(view),
            revision,
            renderer.entities(),
        );
        let id = renderer.entity_at(at).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(id
            .and_then(|id| self.outlines.channels.target_of(id))
            .map(str::to_owned))
    }

    /// Draws the Scene's image on screen and the overlays for `state` at `extent` physical
    /// pixels (`scale` per logical px), and presents it into `layer`. `state` carries the
    /// camera and `gizmo` the drag pose of the frame on screen, so overlays never drift from
    /// a lagging image. A raster Scene draws selection outlines itself: changed outlines go
    /// to the renderer and `scene` redraws. False skips the frame while the last one is
    /// still on the GPU: call again on a later pass.
    /// # Safety
    /// `layer` is the Scene view's live native layer (`present::Target`).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn frame(
        &mut self,
        renderer: &mut ara_render::Renderer,
        scene: &mut crate::app::viewport::SceneView,
        view: &DocumentView,
        state: &EditorState,
        changed: &std::collections::BTreeMap<u64, &str>,
        gizmo: GizmoState,
        revision: u64,
        layer: super::present::Target,
        extent: [u32; 2],
        scale: f32,
    ) -> Result<bool> {
        if !self
            .ui
            .wait_previous_for(std::time::Duration::from_millis(1))?
        {
            return Ok(false);
        }
        if self.light != state.light {
            self.sheet = sheet(state.light)?;
            self.light = state.light;
        }
        // The image on screen: the last completed frame of an asynchronous Scene, or the
        // published presentation of a synchronous one (as the legacy window attaches it).
        scene
            .attach(&mut self.ui, renderer, state.quality)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.outlines.in_scene = scene.draws_outlines();
        let logical = [extent[0] as f32 / scale, extent[1] as f32 / scale];
        let rect = Box2 {
            x: 0.0,
            y: 0.0,
            w: logical[0],
            h: logical[1],
        };
        let overlays = overlay::build(
            view,
            state,
            rect,
            renderer,
            changed,
            gizmo,
            &mut self.outlines,
            revision,
            &self.sheet,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(outlines) = self.outlines.take_scene_outlines() {
            renderer.set_outlines(outlines);
            scene.redraw();
        }
        self.outlines
            .upload(&mut self.paint, &mut self.ui)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.ui.set_dpi(scale)?;
        self.ui.overlays(rect, overlay::ENTITY_IMAGE, overlays);
        let list = DisplayList {
            commands: vec![Draw::Image {
                node: 1,
                rect,
                image: overlay::viewport_image(),
                tint: Color(255, 255, 255, 255),
            }],
            glyphs: Vec::new(),
        };
        // Metal: the adapter has its own queue (Vulkan shares the renderer's). A synchronous
        // frame's image needs ordering after the renderer's work. An asynchronous Scene shows
        // its last completed frame instead, so waiting on the renderer would wait on the
        // frame still in flight and stall the UI until it lands (seconds while converging).
        #[cfg(target_os = "macos")]
        if !renderer.scene_frame_async() {
            self.ui.order_after(renderer.gpu());
        }
        let size = [
            logical[0].round().max(1.0) as u32,
            logical[1].round().max(1.0) as u32,
        ];
        super::trace::time("scene paint", std::time::Duration::from_millis(3), || {
            self.paint
                .render(&mut self.ui, &list, &[], size, TextMode::Raster)
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let present = match self.present.take() {
            Some((mut present, was)) => {
                if was != extent {
                    super::present::resize(&mut present, extent)?;
                }
                present
            }
            // SAFETY: forwarded caller contract.
            None => unsafe { super::present::create(&self.ui, renderer, layer, extent)? },
        };
        let (present, _) = self.present.insert((present, extent));
        // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
        super::trace::time("scene present", std::time::Duration::from_millis(3), || {
            // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
            unsafe { self.ui.present(ui_device(renderer.gpu()), present) }
        })?;
        Ok(true)
    }

    /// Releases the layer presentation; call before the window closes.
    pub fn retire(&mut self, renderer: &ara_render::Renderer) {
        let _ = self.ui.wait_idle(ui_device(renderer.gpu()));
        if let Some((mut present, _)) = self.present.take() {
            // SAFETY: the adapter is idle and the presentation is not used again.
            unsafe {
                self.ui
                    .retire_present(ui_device(renderer.gpu()), &mut present)
            };
        }
    }
}

/// The overlay colours come from the editor theme's tokens.
fn sheet(light: bool) -> Result<ara_ui::style::Sheet> {
    let theme = if light {
        ara_ui::theme::EditorTheme::Light
    } else {
        ara_ui::theme::EditorTheme::Dark
    };
    let tokens = theme.tokens().map_err(|e| anyhow::anyhow!("{e}"))?;
    ara_ui::style::Sheet::with_tokens("", tokens).map_err(|e| anyhow::anyhow!("{e}"))
}
