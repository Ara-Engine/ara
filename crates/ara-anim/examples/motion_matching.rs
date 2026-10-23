//! Scripted locomotion and A/B-interleaved SIMD-search/complete-crowd measurements.
mod v2_support;
use ara_anim::{
    glam::{Mat4, Vec3, Vec4},
    graph::*,
    matching::*,
    *,
};
use ara_schema::AssetRef;
use rayon::prelude::*;
use std::{hint::black_box, time::Instant};
fn median(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    (
        samples[samples.len() / 2],
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)],
    )
}
struct Actor {
    animator: Animator,
    model: Vec<Mat4>,
    palette: Vec<Mat4>,
    snapshot: Snapshot,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frames: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(100);
    if frames == 0 {
        return Err("frames must be positive".into());
    }
    let s = v2_support::skeleton(1.0, 65)?;
    let clips = v2_support::locomotion(&s)?;
    let config = MatchConfig {
        bones: vec!["LeftFoot".into(), "RightFoot".into()],
        horizons: vec![9600, 24000],
        ..Default::default()
    };
    let borrowed: Vec<_> = clips
        .iter()
        .enumerate()
        .map(|(i, c)| (v2_support::reference(i), c))
        .collect();
    let db = MotionDatabase::cook(&config, &s, &borrowed)?;
    let n = db.len();
    println!(
        "database rows={n} dimensions={} feature_bytes={}",
        config.dimensions(),
        db.feature_bytes()
    );
    let mut queries: Vec<Vec<Vec4>> = (0..1000)
        .map(|i| db.normalized_row(i % n).expect("row index wraps").to_vec())
        .collect();
    for (i, q) in queries.iter_mut().enumerate() {
        for g in q {
            *g += Vec4::splat((i as f32 * 0.1).sin() * 0.015);
        }
    }
    let mut a = Vec::new();
    let mut b = Vec::new();
    for frame in 0..frames + 10 {
        for exact in if frame % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let start = Instant::now();
            for q in &queries {
                black_box(db.search(black_box(q), if exact { 0 } else { 512 })?);
            }
            let us = start.elapsed().as_secs_f64() * 1000.0;
            if frame >= 10 {
                if exact { a.push(us) } else { b.push(us) }
            }
        }
    }
    let exact = median(&mut a);
    let budget = median(&mut b);
    println!(
        "search_exact_us_per_character p50={:.3} p95={:.3}; search_512_us p50={:.3} p95={:.3}",
        exact.0, exact.1, budget.0, budget.1
    );
    let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
    let mut parallel = Vec::new();
    for frame in 0..frames + 10 {
        let start = Instant::now();
        pool.install(|| {
            queries.par_chunks(16).for_each(|chunk| {
                for query in chunk {
                    black_box(db.search(black_box(query), 0).expect("normalized query"));
                }
            })
        });
        if frame >= 10 {
            parallel.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let parallel = median(&mut parallel);
    println!(
        "search_exact_1000_8workers_ms p50={:.3} p95={:.3}",
        parallel.0, parallel.1
    );
    let names: Vec<_> = (0..config.dimensions())
        .map(|i| format!("feature{i:02}"))
        .collect();
    let mut data = ControllerData {
        initial: "locomotion".into(),
        ..Default::default()
    };
    data.parameters = names
        .iter()
        .map(|name| (name.clone(), Parameter::default()))
        .collect();
    let reference = AssetRef("@locomotion.animation#an_search".into());
    data.nodes.insert(
        "search".into(),
        Node {
            kind: NodeKind::MotionMatching,
            clip: Some(reference.clone()),
            matching: Some(MatchNode {
                query: names.clone(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    data.states.insert(
        "locomotion".into(),
        State {
            node: "search".into(),
            ..Default::default()
        },
    );
    let c = Controller::compile_with_matching(
        &data,
        &s,
        clips
            .into_iter()
            .enumerate()
            .map(|(i, c)| (v2_support::reference(i), c))
            .collect(),
        vec![(reference, db)],
    )?;
    let mut predictor = TrajectoryPredictor::default();
    let mut future = vec![TrajectoryPoint::default(); config.horizons.len()];
    let horizons: Vec<_> = config
        .horizons
        .iter()
        .map(|t| *t as f32 / 48000.0)
        .collect();
    let mut features = FeatureWriter::new(&config, &s)?;
    let mut raw = vec![0.0; config.dimensions()];
    let mut demo = Animator::new(&c, &s)?;
    let mut previous = Pose::new(s.len());
    let mut checksum = 0.0;
    for tick in 0..600 {
        let (velocity, yaw) = match tick / 100 {
            0 => (Vec3::Z * 1.2, 0.0),
            1 => (Vec3::Z * 3.2, 0.0),
            2 => (Vec3::X * 1.2, 0.0),
            3 => (-Vec3::X * 1.2, 0.0),
            4 => (Vec3::X * 1.2, 1.2),
            _ => (Vec3::ZERO, 1.2),
        };
        predictor.step(velocity, yaw, 0.08, 1.0 / 60.0)?;
        predictor.predict(velocity, yaw, 0.08, &horizons, 1.0 / 60.0, &mut future)?;
        features.write(
            &s,
            demo.pose(),
            &previous,
            1.0 / 60.0,
            predictor.motion.value,
            &future,
            &mut raw,
        )?;
        previous.copy_from(demo.pose())?;
        for (name, value) in names.iter().zip(&raw) {
            demo.set(
                c.parameter(name).expect("query parameter"),
                ParameterValue::Float(*value),
            )?;
        }
        checksum += demo.tick(&c, &s, |_| {})?.translation.length();
    }
    println!("scripted_demo_ticks=600 root_distance_checksum={checksum:.6}");
    let mut actors: Vec<_> = (0..1000)
        .map(|i| {
            let mut animator = Animator::new(&c, &s)?;
            for (j, name) in names.iter().enumerate() {
                animator.set(
                    c.parameter(name).expect("query parameter"),
                    ParameterValue::Float(raw[j] + (i % 5) as f32 * 0.01),
                )?;
            }
            let snapshot = animator.snapshot();
            Ok(Actor {
                animator,
                model: vec![Mat4::IDENTITY; s.len()],
                palette: vec![Mat4::IDENTITY; s.len()],
                snapshot,
            })
        })
        .collect::<Result<_, Error>>()?;
    let mut samples = Vec::new();
    for frame in 0..frames + 10 {
        let start = Instant::now();
        pool.install(|| {
            actors.par_chunks_mut(16).for_each(|chunk| {
                for actor in chunk {
                    black_box(
                        actor
                            .animator
                            .tick(&c, &s, |_| {})
                            .expect("compiled motion graph"),
                    );
                    actor.animator.snapshot_into(&mut actor.snapshot);
                    pose::model_matrices(&s, actor.animator.pose(), &mut actor.model)
                        .expect("model scratch");
                    pose::skinning_palette(&s, &actor.model, &mut actor.palette)
                        .expect("palette scratch");
                    black_box(&actor.palette);
                }
            })
        });
        if frame >= 10 {
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let crowd = median(&mut samples);
    println!(
        "motion_crowd_1000x65_8workers_ms p50={:.3} p95={:.3}",
        crowd.0, crowd.1
    );
    Ok(())
}
