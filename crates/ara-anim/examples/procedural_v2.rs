//! Bounded procedural-solver and cross-proportion retarget measurements.
mod v2_support;
use ara_anim::{
    glam::{Mat4, Vec3},
    ik::*,
    retarget::*,
    *,
};
use std::{hint::black_box, time::Instant};
fn run(mut operation: impl FnMut() -> Result<(), Error>) -> Result<(f64, f64), Error> {
    let mut samples = Vec::new();
    for round in 0..40 {
        let start = Instant::now();
        for _ in 0..200 {
            operation()?;
        }
        if round >= 5 {
            samples.push(start.elapsed().as_secs_f64() * 1e6 / 200.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    Ok((
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100],
    ))
}
struct Flat;
impl GroundQuery for Flat {
    fn probe(&mut self, origin: Vec3, displacement: Vec3) -> Result<Option<GroundHit>, Error> {
        let t = -origin.y / displacement.y;
        Ok((0.0..=1.0).contains(&t).then_some(GroundHit {
            point: origin + displacement * t,
            normal: Vec3::Y,
        }))
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let s = v2_support::skeleton(1.0, 65)?;
    let target = v2_support::skeleton(1.3, 65)?;
    let arms = ChainData {
        joints: vec![
            "RightUpperArm".into(),
            "RightLowerArm".into(),
            "RightHand".into(),
        ],
        ..Default::default()
    };
    let mut pose = Pose::new(s.len());
    for solver in [ChainSolver::Fabrik, ChainSolver::Ccd] {
        let mut chain = Chain::cook(&s, &arms)?;
        let mut error = 0.0;
        let times = run(|| {
            pose.copy_from(s.bind_pose())?;
            error = chain
                .solve(
                    &s,
                    &mut pose,
                    Vec3::new(0.5, 1.2, 0.15),
                    solver,
                    SolverSettings::default(),
                )?
                .error;
            Ok(())
        })?;
        println!(
            "{solver:?} 65_bone_us p50={:.3} p95={:.3} residual_mm={:.4}",
            times.0,
            times.1,
            error * 1000.0
        );
    }
    let left = ChainData {
        joints: vec![
            "LeftUpperLeg".into(),
            "LeftLowerLeg".into(),
            "LeftFoot".into(),
        ],
        ..Default::default()
    };
    let right = ChainData {
        joints: vec![
            "RightUpperLeg".into(),
            "RightLowerLeg".into(),
            "RightFoot".into(),
        ],
        ..Default::default()
    };
    let mut feet = FootPlacement::new(
        &s,
        1,
        &[
            Foot {
                chain: left.clone(),
                ..Default::default()
            },
            Foot {
                chain: right.clone(),
                ..Default::default()
            },
        ],
        FootSettings {
            sole_height: 0.04,
            ..Default::default()
        },
    )?;
    let times = run(|| {
        pose.copy_from(s.bind_pose())?;
        black_box(feet.solve(&s, &mut pose, RootMotion::default(), &mut Flat)?);
        Ok(())
    })?;
    println!(
        "foot_placement_2feet_65bones_us p50={:.3} p95={:.3}",
        times.0, times.1
    );
    let mut body = FullBody::new(&s, 1, &[arms, left, right], 3)?;
    let goals = [
        BodyGoal {
            chain: 0,
            position: Vec3::new(0.5, 1.2, 0.15),
            rotation: None,
            priority: 0,
            pelvis_weight: 0.1,
        },
        BodyGoal {
            chain: 1,
            position: Vec3::new(-0.1, 0.04, 0.0),
            rotation: None,
            priority: 100,
            pelvis_weight: 1.0,
        },
        BodyGoal {
            chain: 2,
            position: Vec3::new(0.1, 0.04, 0.0),
            rotation: None,
            priority: 100,
            pelvis_weight: 1.0,
        },
    ];
    let times = run(|| {
        pose.copy_from(s.bind_pose())?;
        body.solve(&s, &mut pose, &goals, 0.1, SolverSettings::default())
    })?;
    println!(
        "full_body_3goals_65bones_us p50={:.3} p95={:.3}",
        times.0, times.1
    );
    let mut retarget = Retargeter::cook(&s, &RigMap::auto(&s)?, &target, &RigMap::auto(&target)?)?;
    let mut out = Pose::new(target.len());
    let times = run(|| retarget.apply(&s, s.bind_pose(), &target, &mut out, true))?;
    let mut model = vec![Mat4::IDENTITY; target.len()];
    pose::model_matrices(&target, &out, &mut model)?;
    let plant = (model[16].w_axis.y - 0.052).abs() * 1000.0;
    println!(
        "retarget_65bones_1.3x_us p50={:.3} p95={:.3} bind_plant_mm={plant:.5}",
        times.0, times.1
    );
    Ok(())
}
