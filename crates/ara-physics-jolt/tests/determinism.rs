mod support;
use ara_physics::*;
use ara_physics_jolt::JoltWorld;
use support::*;

struct Scene {
    world: JoltWorld,
    bodies: Vec<BodyHandle>,
    platform: BodyHandle,
    walker: CharacterHandle,
    rider: CharacterHandle,
    car: VehicleHandle,
}
fn platform(t: f32) -> Vec3 {
    Vec3::new(
        -15.0,
        1.0 + 0.4 * (t * 1.7).sin(),
        10.0 + 3.0 * (t * 1.1).sin(),
    )
}
impl Scene {
    fn new(threads: u32, perturb: bool) -> Self {
        let mut w = world(threads, 2048);
        ground(&mut w, 100.0);
        bowl(&mut w, Vec3::ZERO, 8.0, 4.0);
        stairs(&mut w, -20.0, -10.0);
        let mut bodies = grid(&mut w, 800, Vec3::Y * 6.0, 10, 42);
        if perturb {
            let s = w.backend().body_state(bodies[0]).expect("control body");
            let mut p = s.pose;
            p.position[0] = f32::from_bits(p.position[0].to_bits() + 1);
            w.apply(&[BodyCommand::Teleport {
                body: bodies[0],
                pose: p,
            }])
            .expect("one-bit negative control");
        }
        for layer in 0..5 {
            let n = 5 - layer;
            for x in 0..n {
                for z in 0..n {
                    bodies.push(add_box(
                        &mut w,
                        Vec3::new(
                            20.0 + x as f32 - (n as f32 - 1.0) * 0.5,
                            0.5 + layer as f32,
                            z as f32 - (n as f32 - 1.0) * 0.5,
                        ),
                        Quat::IDENTITY,
                        Vec3::splat(0.5),
                        MotionType::Dynamic,
                    ));
                }
            }
        }
        let p = add_box(
            &mut w,
            platform(0.0),
            Quat::IDENTITY,
            Vec3::new(1.5, 0.1, 1.5),
            MotionType::Kinematic,
        );
        let car = car(&mut w, Vec3::new(0.0, 1.0, 25.0));
        bodies.push(w.backend().vehicle_state(car).expect("car").chassis);
        let walker = character(&mut w, Vec3::new(-20.0, 0.0, -12.0));
        let rider = character(&mut w, platform(0.0) + Vec3::Y * 0.15);
        w.backend_mut().optimize().expect("broadphase");
        Self {
            world: w,
            bodies,
            platform: p,
            walker,
            rider,
            car,
        }
    }
    fn tick(&mut self, tick: u32) {
        let t = tick as f32 * DT;
        kinematic(&mut self.world, self.platform, platform(t + DT));
        self.world
            .step_character(
                self.walker,
                CharacterInput {
                    crouch: (130..160).contains(&tick),
                    ..input(Vec3::new((t * 0.7).sin(), 0.0, (t * 0.7).cos()) * 2.0)
                },
            )
            .expect("walker");
        self.world
            .step_character(self.rider, CharacterInput::default())
            .expect("rider");
        self.world
            .backend_mut()
            .drive(
                self.car,
                VehicleInput {
                    throttle: 1.0,
                    steer: (t * 0.5).sin(),
                    ..Default::default()
                },
            )
            .expect("driver");
        self.world.step().expect("deterministic tick");
    }
    fn hash(&self) -> u64 {
        hash(
            &self.world,
            &self.bodies,
            &[self.walker, self.rider],
            &[self.car],
        )
    }
    fn run(&mut self, range: std::ops::Range<u32>) -> Vec<u64> {
        range
            .map(|i| {
                self.tick(i);
                self.hash()
            })
            .collect()
    }
}

#[test]
fn in_process_thread_count_and_negative_control() {
    let baseline = Scene::new(1, false).run(0..240);
    assert_eq!(Scene::new(1, false).run(0..240), baseline);
    assert_eq!(
        Scene::new(4, false).run(0..240),
        baseline,
        "four-thread invariance"
    );
    assert_eq!(
        Scene::new(8, false).run(0..240),
        baseline,
        "eight-thread invariance"
    );
    assert_ne!(
        Scene::new(1, true).run(0..240),
        baseline,
        "negative control must be sensitive to a changed float bit"
    );
    println!(
        "240-tick body/character/drivetrain hash at 1/4/8 threads: {:016x}",
        baseline[239]
    );
}

#[test]
fn snapshot_restores_every_tick_and_fresh_world_including_character_shape() {
    let mut scene = Scene::new(4, false);
    scene.run(0..145);
    let snapshot = scene.world.snapshot().expect("tick 145");
    let first = scene.run(145..240);
    scene.world.restore(&snapshot).expect("restore same world");
    assert_eq!(scene.world.tick(), 145);
    assert_eq!(scene.run(145..240), first);
    let mut fresh = Scene::new(1, false);
    fresh
        .world
        .restore(&snapshot)
        .expect("restore fresh world with different thread count");
    assert_eq!(
        fresh.run(145..240),
        first,
        "fresh restore retains contacts, suspension, engine, gearbox and crouch shape"
    );
    let mut other = Scene::new(4, false);
    add_box(
        &mut other.world,
        Vec3::Y * 100.0,
        Quat::IDENTITY,
        Vec3::ONE,
        MotionType::Static,
    );
    assert!(
        other.world.restore(&snapshot).is_err(),
        "topology mismatch must fail before restore"
    );
    println!(
        "snapshot at tick145: {} native bytes",
        snapshot.native().byte_len()
    );
}

#[test]
#[ignore = "invoked in a separate process by cross_process_hashes_match"]
fn cross_process_child() {
    assert!(std::env::var_os("ARA_PHYSICS_PROBE").is_some());
    let chain = Scene::new(4, false).run(0..240);
    let digest = chain.iter().fold(14695981039346656037u64, |h, n| {
        n.to_le_bytes()
            .into_iter()
            .fold(h, |v, b| (v ^ u64::from(b)).wrapping_mul(1099511628211))
    });
    println!("ARA_PHYSICS_HASH={digest:016x}");
}

#[test]
fn cross_process_hashes_match() {
    let exe = std::env::current_exe().expect("test executable path");
    let run = || {
        let output = std::process::Command::new(&exe)
            .args(["--exact", "cross_process_child", "--ignored", "--nocapture"])
            .env("ARA_PHYSICS_PROBE", "1")
            .output()
            .expect("spawn isolated physics probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("probe UTF8")
            .lines()
            .find_map(|s| s.strip_prefix("ARA_PHYSICS_HASH=").map(str::to_owned))
            .expect("probe hash output")
    };
    let first = run();
    assert_eq!(first, run());
    println!("cross-process per-tick chain digest: {first}");
}
