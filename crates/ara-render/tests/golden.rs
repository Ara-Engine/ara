#![allow(clippy::unwrap_used)]
#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Result, ensure};
use ara_render::{
    Renderer,
    capture::{ViewMode, compare},
    scene::CameraSpec,
};
use glam::Vec3;
use std::path::Path;
#[test]
fn captures_match_goldens_and_reproduce_entity_picking() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let goldens = root.join("tests/goldens");
    let mut renderer = Renderer::new(
        common::scene()?,
        [640, 360],
        &[],
        root.join("../../.sn/test-render"),
    )?;
    let update = std::env::var("ARA_UPDATE_GOLDENS").as_deref() == Ok("1");
    let mut lit = None;
    for mode in ViewMode::ALL {
        let mut request = common::request([640, 360]);
        request.mode = mode;
        let capture = renderer.capture(&request)?;
        let path = goldens.join(format!("{}.png", mode.name()));
        if update {
            capture.image.save(&path)?;
            if matches!(mode, ViewMode::Lit) {
                capture
                    .marked
                    .as_ref()
                    .unwrap()
                    .save(goldens.join("lit.marks.png"))?;
                ara_gpu::cooked::write_json(&goldens.join("lit.legend.json"), &capture.legend)?;
                ara_gpu::cooked::write_json(&goldens.join("provenance.json"), &capture.metadata)?;
            }
        }
        let reference = image::open(&path)?.to_rgba8();
        let result = compare(&capture.image, &reference, 0.005)?;
        if !result.passed {
            let diff = root.join("../../.sn/golden-diffs");
            std::fs::create_dir_all(&diff)?;
            result
                .diff
                .save(diff.join(format!("{}.png", mode.name())))?;
        }
        ensure!(
            result.passed,
            "{} SSIM error {:.6} exceeds 0.005",
            mode.name(),
            result.score
        );
        if matches!(mode, ViewMode::Lit) {
            ensure!(capture.legend.len() == 14, "A baseline entity disappeared");
            for mark in &capture.legend {
                let [x, y, w, h] = mark.bbox;
                ensure!(
                    (y..y + h)
                        .any(|py| (x..x + w)
                            .any(|px| capture.entity_at(px, py) == Some(mark.entity))),
                    "Legend/pick mismatch"
                );
            }
            ensure!(
                capture.entity_at(0, 0).is_none() && capture.entity_at(9999, 0).is_none(),
                "Invalid background/bounds picking"
            );
            lit = Some(capture.image);
        }
    }
    let repeat = renderer.capture(&common::request([640, 360]))?;
    ensure!(
        repeat.image == lit.unwrap(),
        "Deterministic captures depend on prior history"
    );
    let mut framed = common::request([257, 193]);
    framed.camera = CameraSpec::FrameBounds {
        min: Vec3::new(-5., 0., -4.),
        max: Vec3::new(5., 3., 4.),
        direction: Vec3::new(1., 0.8, 1.),
        padding: 0.1,
        vertical_fov: 0.9,
    };
    framed.settle_frames = 2;
    let capture = renderer.capture(&framed)?;
    ensure!(
        capture.image.dimensions() == (257, 193) && !capture.legend.is_empty(),
        "Odd-resolution bounds capture failed"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
