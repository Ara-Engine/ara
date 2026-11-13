//! A kinematic body moved to a target each tick has a velocity, and friction carries
//! what rests on it.
mod support;
use ara_physics::*;
use support::*;

#[test]
fn a_kinematic_platform_has_a_velocity_and_carries_a_resting_box() {
    let mut w = world(1, 64);
    let platform = add_box(
        &mut w,
        Vec3::new(0.0, 0.0, 0.0),
        Quat::IDENTITY,
        Vec3::new(2.0, 0.25, 2.0),
        MotionType::Kinematic,
    );
    let rider = add_box(
        &mut w,
        Vec3::new(0.0, 0.75, 0.0),
        Quat::IDENTITY,
        Vec3::splat(0.5),
        MotionType::Dynamic,
    );
    w.backend_mut().optimize().expect("broadphase");
    // Let the box settle, then slide the platform 2 m/s along +X for two seconds.
    for _ in 0..30 {
        w.step().expect("settle tick");
    }
    let speed = 2.0;
    for tick in 1..=120 {
        kinematic(
            &mut w,
            platform,
            Vec3::new(speed * DT * tick as f32, 0.0, 0.0),
        );
        w.step().expect("slide tick");
    }
    let moved = w.backend().body_state(platform).expect("live platform");
    assert!(
        (moved.velocity.linear[0] - speed).abs() < 1e-3,
        "the platform reports the speed it was moved at: {:?}",
        moved.velocity.linear
    );
    // The platform starts at full speed, so the box slips while friction brings it up to
    // speed (a fraction of a metre), then travels with the platform.
    let riding = w.backend().body_state(rider).expect("live rider");
    assert!(
        (riding.velocity.linear[0] - speed).abs() < 0.02,
        "the box moves with the platform: {:?}",
        riding.velocity.linear
    );
    let (platform, rider) = (position(&w, platform), position(&w, rider));
    assert!((platform.x - 4.0).abs() < 1e-3, "platform at {platform}");
    assert!(
        platform.x - rider.x < 0.6 && rider.x <= platform.x && (rider.y - 0.75).abs() < 0.05,
        "the box rode along: platform {platform}, rider {rider}"
    );
}
