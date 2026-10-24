//! Same-device Scene quality seam. The raster view retains IDs and capture metadata.
use super::*;
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use ara_runtime_render::fidelity::native::{self, FrameChange, NativeConfig, Prepared};
#[derive(Default)]
pub(super) struct SceneView {
    fidelity: Option<FidelityView>,
    content: Option<crate::world::gltf::FidelityContent>,
    prepared: Option<Prepared>,
    revision: Option<(u64, u64, u64, u64)>,
    failed: Option<(u64, u64, u64, u64)>,
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
        if let Err(error) = self.render_fidelity(renderer, editor, view, settle) {
            eprintln!("Scene fidelity keeps its last good presentation: {error}");
            self.remaining = 0;
            self.revision = Some((
                editor.host.store().revision(),
                editor.extraction_revision,
                editor.review_revision,
                self.content
                    .as_ref()
                    .map_or(0, |content| content.revision()),
            ));
        }
        Ok(())
    }
    fn render_fidelity(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        let size = renderer.size();
        let transport = size;
        if self.content.is_none() {
            self.content = Some(crate::world::gltf::FidelityContent::new(
                editor.host.root(),
            )?);
        }
        let content = self.content.as_mut().ok_or("fidelity sources")?;
        let revision = (
            editor.host.store().revision(),
            editor.extraction_revision,
            editor.review_revision,
            content.revision(),
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
        let mut replaced = false;
        if (self.fidelity.is_none() || reset)
            && (self.failed != Some(revision) || editor.gesture.is_some())
        {
            // A rejected generation retries when its inputs change, rather than
            // repeatedly decoding it while the last good image is still displayed.
            self.failed = Some(revision);
            let store = editor.host.store();
            let frame = content.frame(
                view,
                store.workspace(),
                store.schema(),
                editor.gesture.as_ref(),
                &editor.state.hidden,
            )?;
            let post = PostSettings {
                ev100: Some((1.0 / (1.2 * frame.environment.exposure)).log2()),
                bloom: 0.025,
                fog_density: 0.0,
                ..Default::default()
            };
            let rebuild = self
                .prepared
                .as_mut()
                .map(|prepared| prepared.update_frame(&frame))
                .transpose()?
                .is_none_or(|change| change == FrameChange::Rebuild);
            if rebuild {
                let assets = content.content(&frame, store.workspace(), store.schema())?;
                let mut prepared = native::prepare(&frame, assets, texture_config())?;
                prepared.enable_motion();
                let bank = prepared.textures.fork_sources(texture_config())?;
                let mut next = FidelityView::new_native(
                    renderer.gpu_mut(),
                    &prepared.scene,
                    bank,
                    Settings {
                        render_size: transport,
                        display_size: size,
                        tier: Tier::Rt,
                        mode: Mode::Hybrid,
                        upscaler: Upscaler::Taau,
                        probe_ray_budget: 1024,
                        reflection_cutoff: 0.5,
                        post,
                    },
                    None,
                )?;
                if let Err(error) = next.render(
                    renderer.gpu_mut(),
                    &FrameInput {
                        camera: &editor.state.camera.spec(),
                        tick: self.tick,
                        delta_seconds: 1.0 / 144.0,
                        reset: true,
                        lighting_changed: false,
                        deterministic: false,
                    },
                ) {
                    next.destroy(renderer.gpu_mut())?;
                    return Err(error.into());
                }
                self.tick = self.tick.wrapping_add(1);
                // Publish only after upload and a complete first render succeed.
                if let Some(old) = self.fidelity.replace(next) {
                    old.destroy(renderer.gpu_mut())?;
                }
                self.prepared = Some(prepared);
                replaced = true;
            } else if let (Some(fidelity), Some(prepared)) = (&mut self.fidelity, &self.prepared) {
                fidelity.update_native_scene(renderer.gpu_mut(), &prepared.scene)?;
                fidelity.set_post(post)?;
            }
            self.failed = None;
        }
        if self.fidelity.is_none() {
            self.revision = Some(revision);
            return Ok(());
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
                    reset: replaced && n == 0,
                    lighting_changed: false,
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
        quality
            && self.lit
            && (self.fidelity.is_some() && self.remaining > 0
                || self.content.as_ref().is_some_and(|content| {
                    content.revision() != self.revision.map_or(0, |revision| revision.3)
                }))
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

fn texture_config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1 << 30,
        upload_bytes: 16 << 20,
    }
}
