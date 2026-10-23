//! Reproducible 1000 x 65 CPU workload; run with --release --example crowd -- 200.
#[path = "../tests/common/mod.rs"]
mod common;
use ara_anim::{glam::Mat4, graph::ParameterValue, *};
use rayon::prelude::*;
use std::{hint::black_box, time::Instant};

#[repr(align(64))]
struct Actor {
    animator: Animator,
    model: Vec<Mat4>,
    palette: Vec<Mat4>,
    snapshot: Snapshot,
}
fn frame(actor: &mut Actor, c: &Controller, s: &Skeleton, palette: bool) {
    black_box(
        actor
            .animator
            .tick(c, s, |e| {
                black_box(e);
            })
            .expect("benchmark program is valid"),
    );
    actor.animator.snapshot_into(&mut actor.snapshot);
    if palette {
        pose::model_matrices(s, actor.animator.pose(), &mut actor.model)
            .expect("model buffer matches rig");
        pose::skinning_palette(s, &actor.model, &mut actor.palette)
            .expect("palette buffer matches rig");
        black_box(&actor.palette);
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frames: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(200);
    let workers = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>())
        .transpose()?;
    let chunk = std::env::args()
        .nth(3)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(16);
    if frames == 0 || chunk == 0 || workers == Some(0) {
        return Err("benchmark requires at least one measured frame".into());
    }
    let s = common::skeleton(65);
    let raw = common::clip(&s, 1.0);
    let (clip, report) = CookedClip::cook(&raw, &s, CompressionSettings::default())?;
    println!("runtime clip bytes {}", clip.runtime_bytes());
    println!(
        "compression {report:?}; ratio {:.3}x",
        report.source_bytes as f64 / report.cooked_bytes as f64
    );
    let mut data = common::controller_data();
    // Both states exercise a 1D tree. Recurring cross-fades exercise transition
    // conditions, sync phase and two-lane evaluation throughout the measurement.
    data.states.get_mut("idle").expect("fixture state").node = "locomotion".into();
    data.transitions[0].conditions.clear();
    data.transitions[0].duration = 48000;
    data.transitions.push(ara_anim::graph::Transition {
        id: "return".into(),
        from: "move".into(),
        to: "idle".into(),
        duration: 48000,
        ..Default::default()
    });
    let c = common::controller(&data, &s);
    let speed = c.parameter("speed").expect("fixture speed parameter");
    let make = || -> Result<Vec<Actor>, Error> {
        (0..1000)
            .map(|i| {
                let mut animator = Animator::new(&c, &s)?;
                animator.set(speed, ParameterValue::Float((i % 101) as f32 / 100.0))?;
                // Independent saved phase offsets, rather than identical cache access.
                for _ in 0..i % 60 {
                    animator.tick(&c, &s, |_| {})?;
                }
                let snapshot = animator.snapshot();
                Ok(Actor {
                    animator,
                    model: vec![Mat4::IDENTITY; 65],
                    palette: vec![Mat4::IDENTITY; 65],
                    snapshot,
                })
            })
            .collect()
    };
    println!("mode,threads,characters,bones,p50_ms,p95_ms");
    for palette in [false, true] {
        for threads in [1, 2, 8, 16]
            .into_iter()
            .filter(|t| workers.is_none_or(|w| w == *t))
        {
            let mut actors = make()?;
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()?;
            let mut times = Vec::with_capacity(frames);
            for frame_index in 0..frames + 30 {
                let start = Instant::now();
                if threads == 1 {
                    for actor in &mut actors {
                        frame(actor, &c, &s, palette);
                    }
                } else {
                    pool.install(|| {
                        actors.par_chunks_mut(chunk).for_each(|chunk| {
                            for actor in chunk {
                                frame(actor, &c, &s, palette);
                            }
                        })
                    });
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if frame_index >= 30 {
                    times.push(elapsed);
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{},{threads},1000,65,{:.3},{:.3}",
                if palette {
                    "animation+palette"
                } else {
                    "animation"
                },
                times[times.len() / 2],
                times[times.len() * 95 / 100]
            );
        }
    }
    Ok(())
}
