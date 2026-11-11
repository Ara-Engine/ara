//! `--headless`: render the project's scene offscreen and write `editor-scene.png` to the
//! output folder, with no window (release smoke tests, `ara hub open --headless`).
use ara_render::Renderer;
use ara_render::capture::{CaptureRequest, ViewMode};

use super::setup::{self, Options};
use super::viewport::SceneView;
use crate::Editor;
use crate::world::EditWorldView as _;

/// Opens the project, packages its scene, renders it settled and deterministic, and writes
/// the capture; the window-free counterpart of the Scene view.
pub fn run(options: Options) -> ara_ui::Result<()> {
    std::fs::create_dir_all(&options.output)?;
    let mut editor = Editor::open_with(
        &options.project,
        Some(crate::settings::user_settings_path()?),
    )?;
    setup::prepare(&mut editor, &options)?;
    // No editor tick loop publishes completed imports here: this capture owns one
    // asset-ready boundary.
    let _published_assets = ara_play_host::PublishedAssets::for_project(editor.host.root())
        .map_err(anyhow::Error::msg)?;
    let worker = ara_play_host::PlayConfig::for_project(editor.host.root()).import_worker;
    ara_play_host::package_scene_with_progress(editor.host.root(), &worker, |progress| {
        eprintln!(
            "Scene capture cooking: {}/{} complete ({:.1}s elapsed, {:.1}s since completion)",
            progress.completed,
            progress.total,
            progress.elapsed.as_secs_f64(),
            progress.stalled.as_secs_f64()
        );
        Ok(())
    })
    .map_err(anyhow::Error::msg)?;
    let view = editor.view()?;
    let create_renderer =
        if crate::world::gltf::render_settings(editor.host.store().workspace())?.ray_tracing {
            Renderer::new
        } else {
            // Ray queries where the GPU has them, unused without the opt-in.
            Renderer::new_hybrid
        };
    let mut renderer = create_renderer(
        view.scene(editor.host.root(), &editor.state, None, &mut editor.models)?,
        options.size,
        &[],
        editor.host.root().join(".ara/gpu"),
    )?;
    detect_quality(&mut editor, &renderer);
    let mut scene_view = SceneView::default();
    scene_view.render_capture(&mut renderer, &editor, &view, 64)?;
    scene_view
        .capture(
            &mut renderer,
            &CaptureRequest {
                camera: editor.state.camera.spec(),
                size: options.size,
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 4,
                tick: 0,
                seed: 0,
                marks: Some(16),
            },
        )?
        .write(&options.output.join("editor-scene.png"))?;
    scene_view.destroy(&mut renderer)?;
    editor.host.flush();
    renderer.gpu().assert_clean()?;
    Ok(())
}

/// Picks the quality preset this GPU supports and applies it to the editor's settings.
pub(crate) fn detect_quality(editor: &mut Editor, renderer: &Renderer) {
    use ara_render_fidelity::presets::{Capabilities, QualityPreset};
    editor.state.detected_quality = Some(QualityPreset::detect(&Capabilities::of(renderer.gpu())));
    editor.state.apply_settings();
}
