mod support;
use ara_physics::*;
use support::*;

fn platform(x: f32, vertical: bool, t: f32) -> Vec3 {
    Vec3::new(
        x,
        if vertical {
            1.0 + 0.5 * (std::f32::consts::TAU * t / 3.0).sin()
        } else {
            1.0
        },
        6.0 + 3.0 * (std::f32::consts::TAU * t / 4.0).sin(),
    )
}

#[test]
fn s6_character_seven_lanes() {
    let mut w = world(1, 256);
    ground(&mut w, 60.0);
    let h35 = ramp(&mut w, -22.0, 35.0, 8.0);
    ramp(&mut w, -16.0, 35.0, 8.0);
    ramp(&mut w, -10.0, 50.0, 6.0);
    stairs(&mut w, -4.0, 0.0);
    let platforms = [
        add_box(
            &mut w,
            platform(2.0, false, 0.0),
            Quat::IDENTITY,
            Vec3::new(1.5, 0.1, 1.5),
            MotionType::Kinematic,
        ),
        add_box(
            &mut w,
            platform(6.0, true, 0.0),
            Quat::IDENTITY,
            Vec3::new(1.5, 0.1, 1.5),
            MotionType::Kinematic,
        ),
    ];
    add_box(
        &mut w,
        Vec3::new(16.0, 1.5, 4.0),
        Quat::IDENTITY,
        Vec3::new(4.0, 1.5, 0.25),
        MotionType::Static,
    );
    w.backend_mut().optimize().expect("broadphase");
    let lanes = [
        (Vec3::new(-22.0, 0.0, -2.0), Vec3::Z * 3.0, 300),
        (Vec3::new(-16.0, 0.0, -2.0), Vec3::Z * 3.0, 120),
        (Vec3::new(-10.0, 0.0, -2.0), Vec3::Z * 3.0, 360),
        (Vec3::new(-4.0, 0.0, -2.0), Vec3::Z * 2.0, 240),
        (platform(2.0, false, 0.0) + Vec3::Y * 0.15, Vec3::ZERO, 0),
        (platform(6.0, true, 0.0) + Vec3::Y * 0.15, Vec3::ZERO, 0),
        (
            Vec3::new(13.0, 0.0, 2.0),
            Vec3::new(1.0, 0.0, 1.0).normalize() * 3.0,
            180,
        ),
    ];
    let ids = lanes.map(|(p, _, _)| character(&mut w, p));
    let (mut max_steep, mut stopped_y, mut penetration) = (0.0f32, 0.0f32, f32::MIN);
    let mut drift = [0.0f32; 2];
    let mut gap = [0.0f32; 2];
    let mut reference = [None; 2];
    for tick in 0..360 {
        kinematic(
            &mut w,
            platforms[0],
            platform(2.0, false, (tick + 1) as f32 * DT),
        );
        kinematic(
            &mut w,
            platforms[1],
            platform(6.0, true, (tick + 1) as f32 * DT),
        );
        for (id, (_, v, walk)) in ids.iter().zip(lanes) {
            w.step_character(*id, input(if tick < walk { v } else { Vec3::ZERO }))
                .expect("character step");
        }
        w.step().expect("physics tick");
        let states = ids.map(|id| w.backend().character_state(id).expect("character state"));
        if tick == 119 {
            stopped_y = states[1].position[1];
        }
        max_steep = max_steep.max(states[2].position[1]);
        penetration = penetration.max(states[6].position[2] + 0.3 - 3.75);
        if tick >= 30 {
            for k in 0..2 {
                let rel = Vec3::from_array(states[4 + k].position) - position(&w, platforms[k]);
                let horizontal = Vec3::new(rel.x, 0.0, rel.z);
                let base = *reference[k].get_or_insert(horizontal);
                drift[k] = drift[k].max((horizontal - base).length());
                gap[k] = gap[k].max((rel.y - 0.1).abs());
            }
        }
    }
    let s = ids.map(|id| {
        w.backend()
            .character_state(id)
            .expect("final character state")
    });
    let checks = [
        (s[0].position[1] - h35).abs() < 0.05,
        stopped_y > 1.0 && (stopped_y - s[1].position[1]).abs() < 0.02,
        max_steep < 0.6,
        (s[3].position[1] - 1.8).abs() < 0.05,
        drift[0] < 0.15 && gap[0] < 0.05,
        drift[1] < 0.15 && gap[1] < 0.05,
        penetration < 0.01 && s[6].position[0] - 13.0 > 3.0,
    ];
    println!("character lanes: {checks:?}; platform drift={drift:?}, gap={gap:?}");
    assert_eq!(
        checks, [true; 7],
        "final={s:?}, idle start={stopped_y}, steep={max_steep}, wall={penetration}"
    );
}

#[test]
fn crouch_clearance_jump_push_and_restore() {
    let mut w = world(1, 64);
    ground(&mut w, 20.0);
    let c = character(&mut w, Vec3::ZERO);
    for _ in 0..10 {
        w.step_character(c, CharacterInput::default())
            .expect("land");
        w.step().expect("tick");
    }
    w.step_character(
        c,
        CharacterInput {
            crouch: true,
            ..Default::default()
        },
    )
    .expect("crouch");
    w.step().expect("tick");
    add_box(
        &mut w,
        Vec3::new(0.0, 1.3, 0.0),
        Quat::IDENTITY,
        Vec3::new(1.0, 0.1, 1.0),
        MotionType::Static,
    );
    let snapshot = w.snapshot().expect("crouched checkpoint");
    w.step_character(c, CharacterInput::default())
        .expect("stand request");
    w.step().expect("tick");
    assert!(
        w.backend().character_state(c).expect("stance").crouched,
        "ceiling must block standing"
    );
    w.restore(&snapshot)
        .expect("restore crouch and capsule shape");
    assert!(w.backend().character_state(c).expect("stance").crouched);
    for _ in 0..90 {
        w.step_character(
            c,
            CharacterInput {
                crouch: true,
                ..input(Vec3::Z * 2.0)
            },
        )
        .expect("walk crouched");
        w.step().expect("tick");
    }
    w.step_character(c, CharacterInput::default())
        .expect("stand in clearance");
    w.step().expect("tick");
    assert!(!w.backend().character_state(c).expect("stance").crouched);
    w.step_character(
        c,
        CharacterInput {
            jump_speed: 5.0,
            ..Default::default()
        },
    )
    .expect("jump");
    w.step().expect("tick");
    assert!(w.backend().character_state(c).expect("jump state").velocity[1] > 4.0);
    for _ in 0..90 {
        w.step_character(c, CharacterInput::default())
            .expect("land");
        w.step().expect("tick");
    }
    let p = Vec3::from_array(w.backend().character_state(c).expect("feet").position);
    let box_id = add_box(
        &mut w,
        p + Vec3::new(0.0, 0.35, 1.0),
        Quat::IDENTITY,
        Vec3::splat(0.25),
        MotionType::Dynamic,
    );
    let start = position(&w, box_id);
    for _ in 0..120 {
        w.step_character(c, input(Vec3::Z * 2.0)).expect("push");
        w.step().expect("tick");
    }
    assert!(
        position(&w, box_id).z > start.z + 0.2,
        "character should push a finite-mass body"
    );
}
