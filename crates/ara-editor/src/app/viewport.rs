//! Same-device Scene quality seam. The raster view retains IDs and capture metadata.
use super::*;
use ara_render_fidelity::{
    FidelityView, FrameInput, PostSettings,
    environment::FrameEnvironment,
    presets::{Capabilities, FidelityPlan, Plan, RenderPath},
};
use ara_runtime_render::fidelity::native::{self, FrameChange, NativeConfig, Prepared};
use ara_runtime_render::{PresentationImage, fidelity::vfx::Effects};
#[derive(Default)]
pub(super) struct SceneView {
    fidelity: Option<FidelityView>,
    content: Option<crate::world::gltf::FidelityContent>,
    prepared: Option<Prepared>,
    effects: Option<Effects>,
    published: Option<PresentationImage>,
    plan: Option<FidelityPlan>,
    active: bool,
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
    /// Settle a reproducible capture on the live allocation. Reset lighting history
    /// and the jitter/RNG sequence without holding a second view's GPU descriptors.
    pub(super) fn render_capture(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        self.render(renderer, editor, view, 1)?;
        if !self.active {
            return Ok(());
        }
        if self.failed.is_some() {
            return Err("Scene capture could not prepare the requested content generation".into());
        }
        self.fidelity
            .as_mut()
            .ok_or("Scene capture has no fidelity view")?
            .reset_accumulation(renderer.gpu_mut())?;
        self.tick = 0;
        self.render_fidelity(renderer, editor, view, settle)?;
        Ok(())
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
        self.active = false;
        if !lit_fidelity(editor) {
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
        let choice = crate::world::gltf::render_quality(editor.host.store().workspace())?;
        let plan = Plan::resolve(choice, &Capabilities::of(renderer.gpu()), size)?;
        let RenderPath::Fidelity(settings) = plan.path else {
            return Ok(());
        };
        self.active = true;
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
        let resized = self.plan != Some(settings);
        if reset || resized {
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
            let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
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
                if let Some(fidelity) = &mut self.fidelity {
                    fidelity.replace_native_scene(renderer.gpu_mut(), &prepared.scene, bank)?;
                } else {
                    self.fidelity = Some(FidelityView::new_native(
                        renderer.gpu_mut(),
                        &prepared.scene,
                        bank,
                        settings.settings(look.post(PostSettings::default())),
                        None,
                    )?);
                }
                self.prepared = Some(prepared);
                replaced = true;
            } else if let (Some(fidelity), Some(prepared)) = (&mut self.fidelity, &self.prepared) {
                fidelity.update_native_scene(renderer.gpu_mut(), &prepared.scene)?;
            }
            self.fidelity
                .as_mut()
                .ok_or("fidelity allocation")?
                .set_environment(&look)?;
            let sun = frame.sun.map_or((Vec3::Y, Vec3::ZERO), |(direction, lux)| {
                (Vec3::from_array(direction), Vec3::from_array(lux))
            });
            let sun = (sun.0, look.ground_sun(sun.0, sun.1));
            if self.effects.is_none() {
                self.effects = Some(Effects::new(
                    renderer.gpu_mut(),
                    &[],
                    settings.render_size,
                    sun,
                )?);
            }
            self.effects.as_mut().ok_or("effect allocation")?.update(
                renderer.gpu_mut(),
                &frame.effects,
                sun,
            )?;
            self.failed = None;
        }
        if self.fidelity.is_none() {
            self.revision = Some(revision);
            return Ok(());
        }
        let f = self.fidelity.as_mut().ok_or("fidelity allocation")?;
        if resized {
            f.resize_unpublished(renderer.gpu_mut(), settings.render_size, size)?;
            if let Some(effects) = &mut self.effects {
                effects.resize(renderer.gpu_mut(), settings.render_size)?;
            }
        }
        settings.configure(renderer.gpu_mut(), f)?;
        f.prepare_transport(renderer.gpu_mut())?;
        self.plan = Some(settings);
        for n in 0..settle.max(1) {
            let residency = f.refine_native_textures(renderer.gpu_mut())?;
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
            if let Some(effects) = &mut self.effects
                && !effects.is_empty()
            {
                effects.composite_ticks(
                    renderer.gpu_mut(),
                    f,
                    &editor.state.camera.spec(),
                    self.tick,
                    0,
                )?;
            }
            self.tick = self.tick.wrapping_add(1);
            self.remaining = self.remaining.saturating_sub(1);
            if residency.pending_textures > 0 {
                self.remaining = self.remaining.max(1);
            }
        }
        PresentationImage::publish(&mut self.published, renderer.gpu_mut(), f.output(), size)?;
        self.revision = Some(revision);
        Ok(())
    }
    pub fn needs_frame(&self, quality: bool) -> bool {
        quality
            && self.active
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
            && self.active
            && let Some(image) = &self.published
        {
            // SAFETY: This view and renderer share the live device; all submissions complete and restore CopySource.
            unsafe {
                gpu.external_image(
                    overlay::viewport_image(),
                    renderer.gpu().native_image(image.output())?,
                    renderer.gpu().image_view(image.output())?,
                )?;
            }
        }
        Ok(())
    }
    /// Captures the same completed color the Scene viewport displays; the raster
    /// capture supplies picking/mark metadata only. Render this view first.
    pub(super) fn capture(
        &self,
        renderer: &mut Renderer,
        request: &CaptureRequest,
    ) -> anyhow::Result<ara_render::capture::Capture> {
        let mut capture = renderer.capture(request)?;
        if self.active && matches!(request.mode, ViewMode::Lit) {
            self.published
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Scene has no completed fidelity frame"))?
                .capture(renderer, &mut capture)?;
        }
        Ok(capture)
    }
    pub fn destroy(self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        if let Some(image) = self.published {
            image.destroy(renderer.gpu_mut())?;
        }
        if let Some(effects) = self.effects {
            effects.destroy(renderer.gpu_mut())?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_captures_native_quality_environment_and_effects_after_resize() -> anyhow::Result<()> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/renderer-unification/scene-view")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(root.join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Scene presentation test","main":"levels/main.scene"}),
            ),
        )?;
        std::fs::write(
            root.join("levels/main.scene"),
            ara_json::to_canonical_string(&json!({
                "$schema":"ara://schema/scene/1","id":"sc_nativeview","records":{
                    "e_box":{"name":"Box","Transform":{},"MeshRenderer":{"color":[0.15,0.5,0.2,1]}},
                    "e_sky":{"SkyAtmosphere":{},"PostProcess":{"exposure":"manual","ev100":12}},
                "e_sun":{"Transform":{"rotation":[-45,0,0]},"Light":{"kind":"directional","intensity":24000}},
                    "e_fire":{"Transform":{"position":[1,0,0]},"VisualEffect":{"effect":"campfire","scale":0.3,"intensity":0.2}}
                }
            })),
        )?;
        // No source assets: initialize an empty publication without launching a worker.
        let _assets = ara_play_host::assets::Assets::new(&root, &root.join("unused-worker"))
            .map_err(anyhow::Error::msg)?;
        let mut editor = Editor::open(&root)?;
        let view = editor.view()?;
        let mut renderer = Renderer::new(
            view.scene(editor.host.root(), &editor.state, None, &mut editor.models)?,
            [192, 108],
            &[],
            root.join("gpu"),
        )?;
        let mut scene = SceneView::default();
        let result = (|| -> anyhow::Result<()> {
            scene
                .render(&mut renderer, &editor, &view, 2)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert!(scene.active);
            let fidelity = scene.fidelity.as_ref().expect("native Scene view");
            assert_eq!(fidelity.settings().render_size, [96, 54]);
            assert_eq!(fidelity.settings().post.ev100, Some(12.));
            assert!(!scene.effects.as_ref().expect("Scene effects").is_empty());
            let request = CaptureRequest {
                camera: editor.state.camera.spec(),
                size: renderer.size(),
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 1,
                tick: 0,
                seed: 7,
                marks: None,
            };
            let capture = scene.capture(&mut renderer, &request)?;
            capture.write(&root.join("scene-high.png"))?;
            assert!(capture.image.pixels().any(|pixel| pixel[0] != pixel[1]));
            scene
                .render_capture(&mut renderer, &editor, &view, 8)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let first = scene.capture(&mut renderer, &request)?;
            scene
                .render(&mut renderer, &editor, &view, 5)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            scene
                .render_capture(&mut renderer, &editor, &view, 8)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let repeated = scene.capture(&mut renderer, &request)?;
            assert_eq!(
                first.image, repeated.image,
                "capture must reset history on the live view without another descriptor set"
            );
            renderer.resize([240, 136])?;
            scene
                .render(&mut renderer, &editor, &view, 2)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert_eq!(
                scene.published.as_ref().expect("completed image").size(),
                [240, 136]
            );
            // Interactive resizes retain the published frame while releasing the
            // old fidelity targets before allocating their replacements.
            for size in [[192, 108], [256, 144], [240, 136]] {
                renderer.resize(size)?;
                scene
                    .render(&mut renderer, &editor, &view, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert_eq!(
                    scene.published.as_ref().expect("resized frame").size(),
                    size
                );
                assert_eq!(
                    scene
                        .fidelity
                        .as_ref()
                        .expect("live view")
                        .settings()
                        .display_size,
                    size
                );
            }
            // Opening another document reuses this same view; it must not acquire
            // a second set of storage-image descriptors beside the old one.
            let created = editor.apply(
                vec![json!({"cmd":"doc.create","args":{
                    "path":"levels/reopened.scene","kind":"scene"
                }})],
                "Create reopened scene fixture",
                None,
            )?;
            let doc = created["outputs"][0]["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("created scene"))?;
            editor.apply_to(doc, vec![json!({"cmd":"entity.create","args":{
                "name":"Reopened box","components":{"Transform":{},"MeshRenderer":{"color":[0.3,0.6,0.2,1]}}
            }})], "Populate reopened scene fixture")?;
            for path in ["levels/reopened.scene", "levels/main.scene"] {
                editor.action("editor.scene.open", json!({"scene":path}))?;
                let reopened = editor.view()?;
                renderer.replace_scene(reopened.scene(
                    editor.host.root(),
                    &editor.state,
                    None,
                    &mut editor.models,
                )?)?;
                scene
                    .render(&mut renderer, &editor, &reopened, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert!(scene.active);
                assert!(
                    scene.failed.is_none(),
                    "scene reopen must publish without heap exhaustion"
                );
            }
            renderer.gpu().assert_clean()?;
            Ok(())
        })();
        scene
            .destroy(&mut renderer)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        result
    }
}
