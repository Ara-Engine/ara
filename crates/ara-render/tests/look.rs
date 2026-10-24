#![allow(clippy::unwrap_used)]
#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Result, ensure};
use ara_render::{
    Renderer,
    capture::compare,
    look::{
        AmbientOcclusion, AutoExposure, FarShadows, Fog, Look, Quality, Reflections, Shadows, Sky,
        gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS},
    },
};
use std::path::Path;

/// Every raster look feature at once (physical sky, fog, metering, bloom, four
/// cascades, GTAO, reflections, probe GI), from the first frame (no history) through steady
/// frames and a resize, with validation (including synchronization) clean. Lit
/// captures stay finite, visibly lit and different from the default look.
#[test]
fn every_raster_look_feature_renders_validation_clean() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut renderer = Renderer::new(
        common::scene()?,
        [320, 180],
        &[],
        root.join("../../.sn/test-render-look"),
    )?;
    let request = common::request([320, 180]);
    let plain = renderer.capture(&request)?;
    renderer.set_look(Look {
        auto_exposure: Some(AutoExposure::default()),
        bloom: 0.05,
        sky: Some(Sky::default()),
        fog: Some(Fog::default()),
        ..Look::default()
    })?;
    renderer.set_quality(Quality {
        shadows: Shadows {
            cascades: 4,
            resolution: 1024,
            distance: 80.,
            far: Some(FarShadows {
                resolution: 512,
                distance: 400.,
            }),
        },
        ambient_occlusion: Some(AmbientOcclusion::default()),
        reflections: Some(Reflections::default()),
        gi_budget: 256,
        bloom: true,
    })?;
    // A 2x2x2 lattice whose probes see only sky: relight, atlases and gather run.
    let probes = 8;
    let mut statics = vec![[1., 1., 0., 0.]; probes * STATIC_VECTORS];
    for p in 0..probes {
        statics[p * STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        origin: [-6., 0.5, -6.],
        spacing: [12., 6., 12.],
        counts: [2, 2, 2],
        texels: vec![[255 << 24, 0, 0, 0]; probes * SURFACE_TEXELS],
        statics,
    }))?;
    for _ in 0..3 {
        renderer.render(&request.camera, request.exposure)?;
    }
    renderer.resize([256, 144])?;
    renderer.render(&request.camera, request.exposure)?;
    let looked = renderer.capture(&request)?;
    let pixels = looked.image.as_raw();
    let mean = pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / pixels.len() as f64;
    ensure!(mean > 10., "the full look rendered almost black ({mean})");
    ensure!(
        !compare(&looked.image, &plain.image, 0.005)?.passed,
        "the look made no visible difference"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
