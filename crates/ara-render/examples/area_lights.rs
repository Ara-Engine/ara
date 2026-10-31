//! GPU cost of local light kinds: the common scene at 2560x1440 lit by a 4x4 grid of
//! sixteen lights of one kind (`point`, `sphere`, `tube`, `rect`, `disc`, or `none`),
//! 120 warm frames; prints each pass's p50/p95 as JSON and writes it under
//! `target/area-lights/`. Wrap timing runs: `cargo run -p ara-dev -- gpu -- cargo run
//! -p ara-render --release --example area_lights -- rect`.
mod common;
use anyhow::{Result, bail};
use ara_render::{
    Renderer,
    scene::{LIGHT_DISC, LIGHT_RECT, Light},
};
use std::{collections::BTreeMap, path::Path};

fn main() -> Result<()> {
    let kind = std::env::args().nth(1).unwrap_or_else(|| "point".into());
    let mut scene = common::scene()?;
    scene.lights = (0..16)
        .map(|i| {
            let (x, z) = ((i % 4) as f32 * 3. - 4.5, (i / 4) as f32 * 3. - 4.5);
            let mut light = Light {
                position_range: [x, 2.5, z, 6.],
                color_lumens: [1., 0.9, 0.8, 1500.],
                direction_outer: [0., -1., 0., 0.],
                tangent_length: [1., 0., 0., 0.],
                ..Light::ZERO
            };
            match kind.as_str() {
                "point" | "none" => {}
                "sphere" => light.parameters[2] = 0.15,
                "tube" => {
                    light.parameters[2] = 0.05;
                    light.tangent_length[3] = 1.2;
                }
                "rect" => {
                    light.parameters[0] = LIGHT_RECT;
                    light.rect = [0.6, 0.3, 0.1, 0.15];
                }
                "disc" => {
                    light.parameters[0] = LIGHT_DISC;
                    light.parameters[2] = 0.3;
                }
                other => bail!("unknown kind {other}"),
            }
            Ok(light)
        })
        .collect::<Result<_>>()?;
    if kind == "none" {
        scene.lights.clear();
    }
    let request = common::request([2560, 1440]);
    let mut renderer = Renderer::new(scene, request.size, &[], ".sn/gpu")?;
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    for i in 0..128 {
        renderer.render(&request.camera, request.exposure)?;
        if i >= 8 {
            for timing in renderer.timings() {
                samples
                    .entry(timing.pass.clone())
                    .or_default()
                    .push(timing.milliseconds);
            }
        }
    }
    let mut passes = BTreeMap::new();
    for (name, mut times) in samples {
        times.sort_by(f64::total_cmp);
        passes.insert(
            name,
            serde_json::json!({"p50_ms": times[times.len() / 2], "p95_ms": times[times.len() * 95 / 100]}),
        );
    }
    let report = serde_json::json!({"kind": kind, "size": request.size, "passes": passes});
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/area-lights");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{kind}.json")),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string(&report)?);
    renderer.gpu().assert_clean()?;
    Ok(())
}
