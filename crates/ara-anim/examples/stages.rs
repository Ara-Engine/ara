//! Same-assets sampling/blending/matrix kernels, shared with the ozz harness.
#[path = "../tests/common/mod.rs"]
mod common;
use ara_anim::{glam::Mat4, *};
use rayon::prelude::*;
use std::{hint::black_box, time::Instant};

#[repr(align(64))]
struct Actor {
    samplers: [Sampler; 4],
    sampled: [Pose; 4],
    lanes: [Pose; 2],
    output: Pose,
    model: Vec<Mat4>,
    palette: Vec<Mat4>,
    phase: usize,
    weight: f32,
}
fn sample(a: &mut Actor, clips: &[CookedClip; 2], s: &Skeleton, frame: usize) {
    let tick = ((frame + a.phase + 1) % 60) as u32 * 800;
    for i in 0..4 {
        a.samplers[i]
            .sample(&clips[i % 2], s, tick, &mut a.sampled[i], &mut [])
            .expect("fixture buffers match");
    }
}
fn blend(a: &mut Actor, frame: usize) {
    for lane in 0..2 {
        a.lanes[lane]
            .copy_from(&a.sampled[lane * 2])
            .expect("same rig");
        a.lanes[lane]
            .blend(&a.sampled[lane * 2 + 1], a.weight, None)
            .expect("valid weight");
    }
    a.output.copy_from(&a.lanes[0]).expect("same rig");
    a.output
        .blend(
            &a.lanes[1],
            ((frame + a.phase) % 60 + 1) as f32 / 60.0,
            None,
        )
        .expect("valid weight");
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let s = common::skeleton(65);
    let mut sources = [common::clip(&s, 1.0), common::clip(&s, 2.0)];
    if args.first().is_some_and(|v| v == "export") {
        let path = args.get(1).ok_or("export requires a destination")?;
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({"skeleton":s.data(), "clips":sources}))?,
        )?;
        return Ok(());
    }
    // The common ozz comparison covers skeletal kernels. Crowd retains curves,
    // notifies, root extraction, the controller and snapshot copying.
    for source in &mut sources {
        source.curves.clear();
    }
    let frames = args
        .first()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(300);
    let threads = args
        .get(1)
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(1);
    let stage = args.get(2).map_or("all", String::as_str);
    let chunk = args
        .get(3)
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(16);
    if frames == 0
        || threads == 0
        || chunk == 0
        || !["sample", "blend", "model", "palette", "all"].contains(&stage)
    {
        return Err("invalid frames, threads, stage or chunk".into());
    }
    let clips = [
        CookedClip::cook(&sources[0], &s, CompressionSettings::default())?.0,
        CookedClip::cook(&sources[1], &s, CompressionSettings::default())?.0,
    ];
    let mut actors: Vec<_> = (0..1000)
        .map(|i| {
            let mut a = Actor {
                samplers: std::array::from_fn(|j| Sampler::new(&clips[j % 2])),
                sampled: std::array::from_fn(|_| Pose::new(65)),
                lanes: std::array::from_fn(|_| Pose::new(65)),
                output: Pose::new(65),
                model: vec![Mat4::IDENTITY; 65],
                palette: vec![Mat4::IDENTITY; 65],
                phase: i % 60,
                weight: (i % 101) as f32 / 100.0,
            };
            sample(&mut a, &clips, &s, 0);
            blend(&mut a, 0);
            pose::model_matrices(&s, &a.output, &mut a.model).expect("fixture rig");
            a
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let mut times = Vec::with_capacity(frames);
    for frame in 0..frames + 30 {
        let run = |a: &mut Actor| {
            if stage == "sample" || stage == "all" {
                sample(a, &clips, &s, frame);
            }
            if stage == "blend" || stage == "all" {
                blend(a, frame);
            }
            if stage == "model" || stage == "all" {
                pose::model_matrices(&s, &a.output, &mut a.model).expect("fixture rig");
            }
            if stage == "palette" || stage == "all" {
                pose::skinning_palette(&s, &a.model, &mut a.palette).expect("fixture rig");
            }
            black_box(&*a);
        };
        let start = Instant::now();
        if threads == 1 {
            actors.iter_mut().for_each(run);
        } else {
            pool.install(|| {
                actors
                    .par_chunks_mut(chunk)
                    .for_each(|chunk| chunk.iter_mut().for_each(&run))
            });
        }
        if frame >= 30 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{stage},{threads},{chunk},{:.6},{:.6}",
        times[frames / 2],
        times[frames * 95 / 100]
    );
    Ok(())
}
