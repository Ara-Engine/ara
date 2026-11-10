//! Compile naga's MSL and execute it through ara-gpu's existing global heap/recording.
use super::*;
use ara_gpu_replay::Pass;
fn pipelines(gpu: &mut MetalDevice) -> [Pipeline; 3] {
    let mut pipelines = Vec::new();
    for pass in [Pass::Downsample, Pass::Upsample, Pass::Tone] {
        let program = ara_gpu_replay::msl(pass).unwrap();
        let path = crate::replay_tests::artifact_dir("msl");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join(format!("{pass:?}.metal")),
            format!("{}{}", super::tests::ABI, program.source),
        )
        .unwrap();
        let mut entries = Vec::new();
        if let Some(vertex) = program.vertex_entry.as_deref() {
            entries.push((cooked::Stage::Vertex, vertex));
            entries.push((cooked::Stage::Fragment, program.entry.as_str()));
        } else {
            entries.push((cooked::Stage::Compute, program.entry.as_str()));
        }
        pipelines.push(gpu.msl_pipeline_from(
            &program.source,
            &entries,
            &crate::replay_tests::state(pass),
            if matches!(pass, Pass::Tone) {
                None
            } else {
                Some([8, 8, 1])
            },
        ));
    }
    pipelines.try_into().unwrap()
}
#[test]
fn naga_msl_replays_renderlab_post_against_cpu() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let p = pipelines(&mut gpu);
    crate::replay_tests::run(&mut gpu, p).unwrap();
}
#[test]
#[ignore = "GPU timing; run through ara-dev gpu with validation disabled"]
fn naga_post_pass_timing() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let pipelines = pipelines(&mut gpu);
    let mut values = std::collections::BTreeMap::<String, Vec<f64>>::new();
    for i in 0..28 {
        let samples = crate::replay_tests::run(&mut gpu, pipelines).unwrap();
        if i >= 8 {
            for sample in samples {
                values
                    .entry(sample.pass)
                    .or_default()
                    .push(sample.milliseconds);
            }
        }
    }
    #[derive(serde::Serialize)]
    struct Timing {
        pass: String,
        samples: usize,
        median_ms: f64,
        p95_ms: f64,
    }
    let report: Vec<_> = values
        .into_iter()
        .map(|(pass, mut v)| {
            v.sort_by(f64::total_cmp);
            eprintln!(
                "WGSL replay timing {pass}: median_ms={:.6} p95_ms={:.6} samples={}",
                v[v.len() / 2],
                v[v.len() * 95 / 100],
                v.len()
            );
            Timing {
                pass,
                samples: v.len(),
                median_ms: v[v.len() / 2],
                p95_ms: v[v.len() * 95 / 100],
            }
        })
        .collect();
    let path = crate::replay_tests::artifact_dir("timing");
    std::fs::create_dir_all(&path).unwrap();
    cooked::write_json(&path.join("timing.json"), &report).unwrap();
    eprintln!("WGSL timing report: {}", path.display());
}
