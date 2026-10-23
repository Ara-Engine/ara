//! Sample driver: `prepare` regenerates sources from the command script, `capture` writes
//! a PNG + marks + provenance, `benchmark` records 100k-sprite GPU timings, `worker`
//! serves atlas imports out of process.
mod common;
use anyhow::Result;
use ara_render2d::Renderer2d;
use std::{collections::BTreeMap, path::Path};

/// 8 warm-up + 120 measured frames; returns per-pass p50/p95 GPU milliseconds.
fn measure(renderer: &mut Renderer2d, camera: &ara_2d::Camera2d) -> Result<serde_json::Value> {
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    for i in 0..128 {
        renderer.render(camera, 1.)?;
        if i >= 8 {
            for t in renderer.timings() {
                samples
                    .entry(t.pass.to_owned())
                    .or_default()
                    .push(t.milliseconds);
            }
        }
    }
    let mut passes = BTreeMap::new();
    for (name, mut times) in samples {
        times.sort_by(f64::total_cmp);
        passes.insert(
            name,
            serde_json::json!({"p50_ms":times[times.len()/2],"p95_ms":times[times.len()*95/100],"samples":times.len()}),
        );
    }
    Ok(serde_json::to_value(passes)?)
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    match std::env::args().nth(1).as_deref() {
        Some("worker") => ara_asset::serve_worker(&ara_import2d::registry()?)?,
        Some("prepare") => common::fixtures::prepare(root)?,
        Some("benchmark") => {
            let camera = ara_2d::Camera2d {
                zoom: 1.,
                pixels_per_metre: 28.,
                pixel_perfect: false,
                ..Default::default()
            };
            let mut runs = Vec::new();
            for (label, size) in [("2.5px", 0.09), ("14px, ~21x overdraw", 0.5)] {
                let scene = common::benchmark(root, 100_000, size)?;
                let mut renderer =
                    Renderer2d::new(&scene, [1280, 720], root.join("../../.sn/gpu2d"))?;
                runs.push(serde_json::json!({"sprite_size":label,"sprites":renderer.instance_count(),"passes":measure(&mut renderer, &camera)?,"device":renderer.gpu().context().info.name}));
            }
            let profile = if cfg!(debug_assertions) {
                "dev opt-level=1, dependencies=3"
            } else {
                "release"
            };
            let report = serde_json::json!({"size":[1280,720],"instances_per_draw":100000,"vertices_per_instance":6,"runs":runs,"profile":profile,"uploads":"immutable device-local instances; frame constants only per frame","lighting":false,"frames_in_flight":1,"method":"Vulkan timestamps, 8 warm-up + 120 measured frames"});
            std::fs::create_dir_all(root.join("measurements"))?;
            ara_gpu::cooked::write_json(&root.join("measurements/100k-sprites.json"), &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            let (scene, camera) = common::scene(root, None)?;
            let mut renderer = Renderer2d::new(&scene, [640, 360], root.join("../../.sn/gpu2d"))?;
            renderer
                .capture(&camera, common::request([640, 360]))?
                .write(&root.join("captures/platformer.png"))?;
            println!(
                "Captured {} sprite/tile instances",
                renderer.instance_count()
            );
        }
    }
    Ok(())
}
