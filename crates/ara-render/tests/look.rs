#![allow(clippy::unwrap_used)]
#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Context, Result, ensure};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, compare},
    look::{
        AmbientOcclusion, AutoExposure, FarShadows, Fog, Look, Quality, Reflections, Shadows, Sky,
        gi::{ProbeSurfaces, ProbeUpdate, STATIC_VECTORS, SURFACE_TEXELS},
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

/// A seeded exposure carries into a history reset: automatic exposure adapts from
/// the seed at the look's rate instead of snapping to its first meter, and
/// `exposure` reads the applied value back.
#[test]
fn a_seeded_exposure_adapts_from_the_seed() -> Result<()> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/exposure-seed")
        .join(std::process::id().to_string());
    let mut renderer = Renderer::new(common::scene()?, [160, 90], &[], cache)?;
    renderer.set_look(Look {
        auto_exposure: Some(AutoExposure::default()),
        ..Look::default()
    })?;
    let request = common::request([160, 90]);
    assert_eq!(renderer.exposure()?, None);
    renderer.render(&request.camera, request.exposure)?;
    let metered = renderer.exposure()?.context("the first frame meters")?;
    renderer.reset_temporal_history();
    renderer.seed_exposure(metered * 8.)?;
    renderer.render(&request.camera, request.exposure)?;
    let adapted = renderer.exposure()?.context("the seeded frame meters")?;
    ensure!(
        adapted > metered * 4. && adapted < metered * 8.,
        "a seeded reset adapts from its seed: {adapted} (meter {metered})"
    );
    Ok(())
}

/// A 2x2x2 lattice whose probes all see a grey (albedo 0.5) surface 1 m away
/// emitting `emission` (f16 bits: 0x6000 is 512); bounces through the volume
/// approach twice that. Distance moments of 100 m reach every shaded point.
fn emissive_surfaces(emission: u32) -> ProbeSurfaces {
    let probes = 8;
    let mut statics = vec![[100., 10_000., 0., 0.]; probes * STATIC_VECTORS];
    for p in 0..probes {
        statics[p * STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    // Packed per ProbeSurfaces::texels: albedo 128 grey, no sky; the octahedral
    // centre normal; the emission and a hit distance of 1 (0x3c00).
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
}

fn mean(image: &image::RgbaImage) -> f64 {
    image.as_raw().iter().map(|&v| f64::from(v)).sum::<f64>() / image.as_raw().len() as f64
}

/// A GI test renderer relighting one probe per frame, and a dimmed request.
fn gi_renderer(name: &str) -> Result<(Renderer, CaptureRequest)> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render")
        .join(name)
        .join(std::process::id().to_string());
    let mut renderer = Renderer::new(common::scene()?, [160, 90], &[], cache)?;
    renderer.set_quality(Quality {
        gi_budget: 1,
        ..Quality::default()
    })?;
    let mut request = common::request([160, 90]);
    request.exposure *= 0.25;
    Ok((renderer, request))
}

/// A capture's probe GI converges within its settle frames whatever the relight
/// budget: with one probe relit per frame, an 8-frame capture matches a 64-frame
/// one instead of stopping after a single, partial bounce.
#[test]
fn a_capture_converges_probe_gi_within_its_settle_frames() -> Result<()> {
    let (mut renderer, mut request) = gi_renderer("gi-converge")?;
    request.settle_frames = 64;
    renderer.set_probe_surfaces(Some(&emissive_surfaces(0)))?;
    let dark = mean(&renderer.capture(&request)?.image);
    renderer.set_probe_surfaces(Some(&emissive_surfaces(0x6000)))?;
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

/// A volume arriving in live frames stays out of shading while it converges (no
/// dark dip as it relights), then fades in converged: the live frame ends up
/// where a converged capture is.
#[test]
fn a_new_volume_fades_into_live_frames_converged() -> Result<()> {
    let (mut renderer, request) = gi_renderer("gi-adoption")?;
    let live = |renderer: &mut Renderer, frames: usize| -> Result<f64> {
        for _ in 0..frames {
            renderer.render(&request.camera, request.exposure)?;
        }
        Ok(mean(&renderer.capture_completed(&request)?.image))
    };
    let sky = live(&mut renderer, 8)?;
    renderer.set_probe_surfaces(Some(&emissive_surfaces(0x6000)))?;
    let arriving = live(&mut renderer, 1)?;
    let converging = live(&mut renderer, 6)?;
    // Eight probes at a one-probe budget need 64 frames for eight bounces,
    // followed by the 30-frame arrival fade.
    let faded = live(&mut renderer, 96)?;
    let converged = mean(&renderer.capture(&request)?.image);
    for (frame, name) in [(arriving, "arriving"), (converging, "converging")] {
        ensure!(
            (frame - sky).abs() < 0.5,
            "a {name} volume stays out of shading: {frame} vs {sky}"
        );
    }
    ensure!(
        (faded - converged).abs() < 1. && (faded - sky).abs() > 2.,
        "the faded-in volume shades converged: {faded} vs {converged} (sky {sky})"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

/// Large updates apply a range of records at a time (live adoption uploads a
/// frame's chunk straight from the delivered update): three ranges in any order
/// light the scene as one whole update does.
#[test]
fn record_ranges_apply_as_the_whole_update() -> Result<()> {
    let (mut renderer, request) = gi_renderer("gi-ranges")?;
    let update = ProbeUpdate::new(&emissive_surfaces(0x6000), None)?;
    renderer.update_probe_surfaces(&update)?;
    let whole = mean(&renderer.capture(&request)?.image);
    renderer.set_probe_surfaces(None)?;
    for range in [5..8, 0..2, 2..5] {
        renderer.update_probe_records(&update, range)?;
    }
    let ranged = mean(&renderer.capture(&request)?.image);
    ensure!(
        (ranged - whole).abs() < 0.01,
        "ranged {ranged} vs whole {whole}"
    );
    ensure!(
        renderer.update_probe_records(&update, 7..9).is_err(),
        "a range past the update is refused"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
