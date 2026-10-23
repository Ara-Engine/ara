//! Same-device Scene quality seam. The raster view retains IDs and capture metadata.
use super::*;
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
#[derive(Default)]
pub(super) struct SceneView {
    fidelity: Option<FidelityView>,
    revision: Option<(u64, u64, u64)>,
    tick: u32,
    remaining: u32,
    camera: Option<crate::state::Camera>,
    /// The fidelity view applies to the Lit mode only; debug views show the raster output.
    lit: bool,
}
impl SceneView {
    pub fn timings(&self) -> &[ara_gpu::commands::PassTiming] {
        self.fidelity
            .as_ref()
            .map(|view| view.timings())
            .unwrap_or(&[])
    }
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        let mode = view_mode(&editor.state.view_mode);
        self.lit = matches!(mode, ViewMode::Lit);
        renderer.render_view(&editor.state.camera.spec(), 0.0015, mode)?;
        if !lit_fidelity(editor) || !renderer.gpu().context().info.ray_tracing {
            return Ok(());
        }
        let size = renderer.size();
        let transport = size;
        let revision = (
            editor.host.store().revision(),
            editor.extraction_revision,
            editor.review_revision,
        );
        if self.camera != Some(editor.state.camera) {
            self.remaining = self.remaining.max(8);
            self.camera = Some(editor.state.camera);
        }
        let reset = self.revision != Some(revision) || editor.gesture.is_some();
        if reset
            || self
                .fidelity
                .as_ref()
                .is_some_and(|f| f.settings().display_size != size)
        {
            self.remaining = 64;
        }
        if self.fidelity.is_none() {
            let scene = view.scene(editor.host.root(), &editor.state, editor.gesture.as_ref())?;
            self.fidelity = Some(FidelityView::new(
                renderer.gpu_mut(),
                &scene,
                Settings {
                    render_size: transport,
                    display_size: size,
                    tier: Tier::Rt,
                    mode: Mode::Hybrid,
                    upscaler: Upscaler::Taau,
                    probe_ray_budget: 1024,
                    reflection_cutoff: 0.5,
                    post: PostSettings {
                        ev100: Some(9.1),
                        bloom: 0.025,
                        fog_density: 0.,
                        ..Default::default()
                    },
                },
                None,
            )?);
        } else if reset {
            let scene = view.scene(editor.host.root(), &editor.state, editor.gesture.as_ref())?;
            if let Some(f) = &mut self.fidelity {
                f.update_scene(renderer.gpu_mut(), &scene)?;
            }
        }
        let f = self.fidelity.as_mut().ok_or("fidelity allocation")?;
        f.resize(renderer.gpu_mut(), transport, size)?;
        for n in 0..settle.max(1) {
            f.render(
                renderer.gpu_mut(),
                &FrameInput {
                    camera: &editor.state.camera.spec(),
                    tick: self.tick,
                    delta_seconds: 1. / 144.,
                    reset: reset && n == 0,
                    lighting_changed: reset && n == 0,
                    deterministic: false,
                },
            )?;
            self.tick = self.tick.wrapping_add(1);
            self.remaining = self.remaining.saturating_sub(1);
        }
        self.revision = Some(revision);
        Ok(())
    }
    pub fn needs_frame(&self, quality: bool) -> bool {
        quality && self.lit && self.fidelity.is_some() && self.remaining > 0
    }
    pub fn attach(
        &self,
        gpu: &mut NativeUi,
        renderer: &Renderer,
        quality: bool,
    ) -> ara_ui::Result<()> {
        super::attach(gpu, renderer)?;
        if quality
            && self.lit
            && let Some(f) = &self.fidelity
        {
            // SAFETY: This view and renderer share the live device; all submissions complete and restore CopySource.
            unsafe {
                gpu.external_image(
                    overlay::viewport_image(),
                    renderer.gpu().native_image(f.output())?,
                    renderer.gpu().image_view(f.output())?,
                )?;
            }
        }
        Ok(())
    }
    pub fn destroy(self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        if let Some(f) = self.fidelity {
            f.destroy(renderer.gpu_mut())?;
        }
        Ok(())
    }
}

/// Viewport View Mode names (the `editor.view` protocol) to renderer debug views.
fn view_mode(name: &str) -> ViewMode {
    match name {
        "baseColor" => ViewMode::Albedo,
        "normals" => ViewMode::Normals,
        "depth" => ViewMode::Depth,
        "roughnessMetal" => ViewMode::RoughnessMetal,
        "entityIds" => ViewMode::EntityIds,
        "shadowCascades" => ViewMode::ShadowCascades,
        _ => ViewMode::Lit,
    }
}
fn lit_fidelity(editor: &Editor) -> bool {
    editor.state.quality && matches!(view_mode(&editor.state.view_mode), ViewMode::Lit)
}
