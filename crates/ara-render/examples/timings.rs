mod common;
use anyhow::Result;
use ara_render::Renderer;
use std::{collections::BTreeMap, path::Path};
fn main() -> Result<()> {
    let request = common::request([1280, 720]);
    let mut renderer = Renderer::new(common::scene()?, request.size, &[], ".sn/gpu")?;
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
    let mut results = BTreeMap::new();
    for (name, mut times) in samples {
        times.sort_by(f64::total_cmp);
        results.insert(name,serde_json::json!({"p50_ms":times[times.len()/2],"p95_ms":times[times.len()*95/100],"samples":times.len()}));
    }
    let report = serde_json::json!({"device":renderer.gpu().info(),"size":request.size,"taa":true,"frames_in_flight":1,"passes":results});
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("captures/timings.json");
    std::fs::create_dir_all(path.parent().expect("Artifact filename has a parent"))?;
    ara_gpu::cooked::write_json(&path, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    renderer.gpu().assert_clean()?;
    Ok(())
}
