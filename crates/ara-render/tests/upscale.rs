#![allow(clippy::unwrap_used)]
#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Result, ensure};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, compare},
};
use std::path::Path;

/// Live frames (jitter and TAA running) until the history has converged, then the
/// completed frame.
fn converged(renderer: &mut Renderer, request: &CaptureRequest) -> Result<image::RgbaImage> {
    for _ in 0..64 {
        renderer.render(&request.camera, request.exposure)?;
    }
    Ok(renderer.capture_completed(request)?.image)
}

/// Temporal upscaling from two-thirds of the display: the resolve reconstructs the
/// display from jittered render samples, so the converged frame is closer to the
/// native frame than the same render size's native frame stretched to the display;
/// captures and picks are in display pixels, through the render pixels under them.
#[test]
fn an_upscaled_frame_resolves_render_samples_to_the_display() -> Result<()> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/upscale")
        .join(std::process::id().to_string());
    let (display, render) = ([240, 135], [160, 90]);
    let mut renderer = Renderer::new(common::scene()?, display, &[], cache)?;
    let request = common::request(display);
    let native = converged(&mut renderer, &request)?;
    let centre = [display[0] / 2, display[1] / 2];
    let picked = renderer.entity_at(centre)?;
    ensure!(picked.is_some(), "the scene covers the view centre");

    renderer.resize(render)?;
    let small = converged(&mut renderer, &common::request(render))?;
    let stretched = image::imageops::resize(
        &small,
        display[0],
        display[1],
        image::imageops::FilterType::Triangle,
    );

    renderer.resize_upscaled(render, display)?;
    assert_eq!((renderer.size(), renderer.render_size()), (display, render));
    let upscaled = converged(&mut renderer, &request)?;
    assert_eq!(upscaled.dimensions(), (display[0], display[1]));
    let (reconstructed, stretched) = (
        compare(&upscaled, &native, 1.)?.score,
        compare(&stretched, &native, 1.)?.score,
    );
    ensure!(
        reconstructed < stretched * 0.8,
        "upscaled 1-SSIM {reconstructed:.4} is not clearly below a stretched render's {stretched:.4}"
    );
    assert_eq!(renderer.entity_at(centre)?, picked);
    renderer.gpu().assert_clean()?;
    Ok(())
}
