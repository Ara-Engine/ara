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
        ..Quality::default()
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

/// A capture's probe GI converges within its settle frames whatever the relight
/// budget. Every probe sees a grey (albedo 0.5) surface 1 m away emitting 512, so
/// bounces through the volume approach twice the emission. With one probe relit
/// per frame, an 8-frame capture matches a 64-frame one instead of stopping
/// after a single, partial bounce.
#[test]
fn a_capture_converges_probe_gi_within_its_settle_frames() -> Result<()> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/gi-converge")
        .join(std::process::id().to_string());
    let mut renderer = Renderer::new(common::scene()?, [160, 90], &[], cache)?;
    renderer.set_quality(Quality {
        gi_budget: 1,
        ..Quality::default()
    })?;
    // Packed per ProbeSurfaces::texels: albedo 128 grey, no sky; the octahedral
    // centre normal; f16 emission (0x6000 is 512) and hit distance 1 (0x3c00).
    let surfaces = |emission: u32| {
        let probes = 8;
        // Distance moments of 100 m: the volume reaches every shaded point.
        let mut statics = vec![[100., 10_000., 0., 0.]; probes * STATIC_VECTORS];
        for p in 0..probes {
            statics[p * STATIC_VECTORS + 64] = [0., 0., 0., 1.];
        }
        let texel = [
            0x0080_8080,
            0x8000_8000,
            emission | emission << 16,
            emission | 0x3c00 << 16,
        ];
        ProbeSurfaces {
            origin: [-6., 0.5, -6.],
            spacing: [12., 6., 12.],
            counts: [2, 2, 2],
            texels: vec![texel; probes * SURFACE_TEXELS],
            statics,
        }
    };
    let mean = |image: &image::RgbaImage| {
        image.as_raw().iter().map(|&v| f64::from(v)).sum::<f64>() / image.as_raw().len() as f64
    };
    let mut request = common::request([160, 90]);
    request.exposure *= 0.25;
    request.settle_frames = 64;
    renderer.set_probe_surfaces(Some(&surfaces(0)))?;
    let dark = mean(&renderer.capture(&request)?.image);
    renderer.set_probe_surfaces(Some(&surfaces(0x6000)))?;
    let settled = mean(&renderer.capture(&request)?.image);
    request.settle_frames = 8;
    let converged = mean(&renderer.capture(&request)?.image);
    ensure!(
        settled > dark + 5.,
        "the probe GI made no visible difference: {settled} vs {dark}"
    );
    ensure!(
        (converged - settled).abs() < 0.5,
        "an 8-frame capture's GI differs from a 64-frame one: {converged} vs {settled}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
