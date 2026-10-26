use super::*;
use crate::{Presenter, QualityChoice, QualityPreset};
use ara_render::capture::ViewMode;
use ara_runtime::extract::EffectView;

#[test]
fn live_tick_advances_and_draws_in_one_particle_graph() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/vfx-tick-batch");
    let mut presenter = Presenter::new([96, 64], Vec::new(), root);
    presenter.set_probe_gi(false);
    presenter.set_quality(QualityChoice::Preset(QualityPreset::Low));
    let mut frame = RenderFrame::default();
    presenter.present(&frame)?;
    assert!(presenter.presentation_error().is_none());
    let renderer = presenter.renderer.as_mut().expect("native renderer");
    let mut effects = Effects::default();
    frame.effects.push(EffectView {
        entity: 1,
        label: "campfire".into(),
        effect: "campfire".into(),
        position: [0.; 3],
        scale: 1.,
        intensity: 1.,
    });
    let camera = crate::camera(&frame)?;
    effects.wait_pipelines(renderer, &frame)?;
    while effects.is_pending() {
        present(&mut effects, renderer, &frame, &camera)?;
    }
    // A frozen redraw, one tick, and bounded catch-up all draw exactly once;
    // catch-up's earlier ticks remain simulation-only graphs.
    let mut simulated = 150;
    for elapsed in [0, 1, 3] {
        frame.tick += elapsed;
        renderer
            .gpu_mut()
            .begin_deferred_completion_on_current_queue()?;
        renderer.set_deferred_post(true);
        renderer.render_view(&camera, 1., ViewMode::Lit)?;
        let rendered = present(&mut effects, renderer, &frame, &camera);
        let mut completion = renderer.gpu_mut().end_deferred_completion()?;
        let passes = renderer.gpu_mut().wait_completion(&mut completion)?;
        rendered?;
        assert_eq!(
            passes.iter().filter(|pass| pass.pass == "Vfx.Draw").count(),
            elapsed.max(1) as usize,
            "the final simulation tick and draw must share one graph"
        );
        simulated += elapsed as u32;
        let system = &effects.live.get(&1).expect("live campfire").system;
        let stats = system.stats(renderer.gpu())?;
        assert_eq!(stats.tick, simulated);
        assert!(stats.particles_alive > 0);
    }
    renderer.gpu().assert_clean()?;
    Ok(())
}

fn present(
    effects: &mut Effects,
    renderer: &mut Renderer,
    frame: &RenderFrame,
    camera: &CameraSpec,
) -> Result<()> {
    let (image, sampled_index, state) = renderer.feature_color();
    let color = SceneColor {
        image,
        sampled_index,
        state,
    };
    let (image, sampled_index, state) = renderer.feature_depth();
    let depth = SceneDepth {
        image,
        sampled_index,
        state,
    };
    effects.present(renderer, frame, camera, color, depth)?;
    Ok(())
}
