//! The editor requests its unchanged display extent before every Scene frame.
use super::*;

#[test]
fn unchanged_display_resize_preserves_upscaled_targets_and_pending_history() -> Result<()> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/resize-upscaled");
    let mut renderer =
        Renderer::new_native(&fixture::scene(), Box::new(Textures), [128, 80], &[], root)?;
    renderer.resize_upscaled([64, 40], [128, 80])?;
    let images = renderer.targets.images.to_vec();
    for _ in 0..8 {
        renderer.resize([128, 80])?;
        assert_eq!(
            renderer.render_size(),
            [64, 40],
            "a display-size notification must not undo the raster plan"
        );
        assert_eq!(
            renderer.targets.images, images,
            "idle frames must retain their GPU allocations"
        );
    }
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    let mut gate = Gate::new(renderer.gpu(), Queue::Graphics)?;
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    let history = renderer.targets.history;
    renderer.resize([128, 80])?;
    assert!(renderer.scene_frame_pending());
    gate.open()?;
    renderer.wait_scene_frame()?;
    assert_eq!(
        renderer.targets.images, images,
        "polling must not adopt a spurious native-size resize"
    );
    assert_eq!(renderer.targets.history, history);
    let mut gate = Gate::new(renderer.gpu(), Queue::Graphics)?;
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    renderer.resize_upscaled([48, 30], [128, 80])?;
    renderer.resize([128, 80])?;
    gate.open()?;
    renderer.wait_scene_frame()?;
    assert_eq!(
        renderer.render_size(),
        [48, 30],
        "window notifications must preserve a queued quality change"
    );
    let scaled_images = renderer.targets.images.to_vec();
    let mut gate = Gate::new(renderer.gpu(), Queue::Graphics)?;
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    renderer.resize([160, 100])?;
    renderer.resize([128, 80])?;
    gate.open()?;
    renderer.wait_scene_frame()?;
    assert_eq!(renderer.size(), [128, 80]);
    assert_eq!(renderer.render_size(), [48, 30]);
    assert_eq!(
        renderer.targets.images, scaled_images,
        "a superseded window resize must not churn targets"
    );
    renderer.resize_upscaled([128, 80], [128, 80])?;
    assert_eq!(
        renderer.render_size(),
        [128, 80],
        "explicit render-size changes still work"
    );
    assert_ne!(renderer.targets.images, images);
    renderer.gpu().assert_clean()?;
    Ok(())
}
