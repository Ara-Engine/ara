mod common;
use anyhow::Result;
use ara_render2d::Renderer2d;
use std::{collections::BTreeMap, path::Path};
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    match std::env::args().nth(1).as_deref() {
        Some("worker") => ara_asset::serve_worker(&ara_import2d::registry()?)?,
        Some("prepare") => common::prepare(root)?,
        Some("benchmark") => {
            let scene = common::benchmark(root)?;
            let mut renderer = Renderer2d::new(&scene, [1280, 720], root.join("../../.sn/gpu2d"))?;
            let camera = ara_2d::Camera2d {
                zoom: 1.,
                pixels_per_metre: 28.,
                pixel_perfect: false,
                ..Default::default()
            };
            let mut samples = BTreeMap::<String, Vec<f64>>::new();
            for i in 0..128 {
                renderer.render(&camera, 1.)?;
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
                passes.insert(name,serde_json::json!({"p50_ms":times[times.len()/2],"p95_ms":times[times.len()*95/100],"samples":times.len()}));
            }
            let report = serde_json::json!({"device":renderer.gpu().context().info,"size":[1280,720],"sprites":renderer.instance_count(),"instances_per_draw":100000,"vertices_per_instance":6,"passes":passes,"profile":"dev opt-level=1, dependencies=3","uploads":"immutable device-local instances; frame constants only per frame","lighting":false,"frames_in_flight":1});
            std::fs::create_dir_all(root.join("measurements"))?;
            ara_gpu::cooked::write_json(&root.join("measurements/100k-sprites.json"), &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            let scene = common::scene(root, None)?;
            let mut renderer = Renderer2d::new(&scene, [640, 360], root.join("../../.sn/gpu2d"))?;
            renderer
                .capture(&common::camera(), common::request([640, 360]))?
                .write(&root.join("captures/platformer.png"))?;
            println!(
                "Captured {} sprite/tile instances",
                renderer.instance_count()
            );
        }
    }
    Ok(())
}
