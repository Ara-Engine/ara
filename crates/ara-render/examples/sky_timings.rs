//! GPU cost of the physical sky (`look::atmosphere`) at 2560x1440 on the example scene:
//! the per-frame passes (sky view, the coarse fog and aerial perspective grid, and the
//! volumetric near volume at the Medium/High/Epic froxel budgets under volumetric height
//! fog), the sky light recapture while the sun moves every frame, and the LUT rebuild
//! while the atmosphere changes every frame. Prints p50/p95 milliseconds per pass.
//! `cargo run -p ara-dev -- gpu -- cargo run -p ara-render --example sky_timings`
mod common;
use anyhow::Result;
use ara_render::{
    Renderer,
    look::{Fog, Look, Quality, Sky, VolumetricFog, VolumetricFogQuality},
};
use glam::Vec3;
use std::collections::BTreeMap;

/// What changes every frame of a case.
#[derive(Clone, Copy)]
enum Change {
    Nothing,
    Sun,
    Atmosphere,
}

fn main() -> Result<()> {
    let size = [2560, 1440];
    let request = common::request(size);
    let mut renderer = Renderer::new(common::scene()?, size, &[], ".sn/gpu")?;
    let near = |tile, slices| Quality {
        volumetric_fog: Some(VolumetricFogQuality {
            tile,
            slices,
            ..VolumetricFogQuality::default()
        }),
        ..Quality::default()
    };
    let fog = Fog {
        volumetric: Some(VolumetricFog::default()),
        ..Fog::default()
    };
    let cases = [
        ("sky", Quality::default(), None, Change::Nothing),
        ("sky+fog medium", near(16, 48), Some(fog), Change::Nothing),
        ("sky+fog high", near(12, 64), Some(fog), Change::Nothing),
        ("sky+fog epic", near(8, 96), Some(fog), Change::Nothing),
        ("sun moving", Quality::default(), None, Change::Sun),
        (
            "atmosphere changing",
            Quality::default(),
            None,
            Change::Atmosphere,
        ),
    ];
    let mut report = BTreeMap::new();
    for (name, quality, fog, change) in cases {
        renderer.set_quality(quality)?;
        let mut samples = BTreeMap::<String, Vec<f64>>::new();
        for i in 0..72u32 {
            let mut sky = Sky::default();
            if let Change::Atmosphere = change {
                sky.atmosphere.multi_scattering = 1. + (i % 2) as f32 * 1e-3;
            }
            renderer.set_look(Look {
                sky: Some(sky),
                fog,
                ..Look::default()
            })?;
            let angle = match change {
                Change::Sun => 0.5 + i as f32 * 0.01,
                _ => 0.5,
            };
            renderer.set_sun(
                Vec3::new(angle.cos(), angle.sin(), 0.3),
                Vec3::splat(120_000.),
            )?;
            renderer.render(&request.camera, 1. / (1.2 * 2f32.powf(14.)))?;
            if i >= 8 {
                for timing in renderer.timings() {
                    let sky_pass = ["Sky.", "Fog."].iter().any(|p| timing.pass.starts_with(p));
                    if sky_pass {
                        samples
                            .entry(timing.pass.clone())
                            .or_default()
                            .push(timing.milliseconds);
                    }
                }
            }
        }
        let passes: BTreeMap<_, _> = samples
            .into_iter()
            .map(|(pass, mut t)| {
                t.sort_by(f64::total_cmp);
                let p50 = t[t.len() / 2];
                let p95 = t[t.len() * 95 / 100];
                (
                    pass,
                    serde_json::json!({"p50_ms": p50, "p95_ms": p95, "samples": t.len()}),
                )
            })
            .collect();
        report.insert(name, passes);
    }
    let report =
        serde_json::json!({"device": renderer.gpu().info(), "size": size, "cases": report});
    println!("{}", serde_json::to_string_pretty(&report)?);
    renderer.gpu().assert_clean()?;
    Ok(())
}
