use super::*;

pub(super) fn run(threads: u32, steps: usize) -> BenchResult<Vec<serde_json::Value>> {
    let load_before = load();
    let mut c = config("game_default", 4096, threads)?;
    c.max_bodies = 4096;
    let scene = |config| -> BenchResult<_> {
        let mut w = JoltWorld::new(config)?;
        ground(&mut w, 150.0);
        let ids = grid(&mut w, 2000, Vec3::new(0.0, 3.0, 0.0), 40, 7);
        let characters = (0..1000)
            .map(|i| {
                character(
                    &mut w,
                    Vec3::new(
                        (i % 40) as f32 * 2.0 - 40.0,
                        0.0,
                        (i / 40) as f32 * 2.0 - 25.0,
                    ),
                )
            })
            .collect::<Vec<_>>();
        w.backend_mut().optimize()?;
        Ok((w, ids, characters))
    };
    let (mut w, ids, characters) = scene(c.clone())?;
    let (mut legacy, legacy_ids, legacy_characters) = scene(c)?;
    legacy.backend_mut().benchmark_character_tree(false);
    let mut times = Vec::with_capacity(steps);
    let mut legacy_times = Vec::with_capacity(steps);
    for tick in 0..steps {
        for side in 0..2 {
            let tree = (side + tick).is_multiple_of(2);
            let (world, actors) = if tree {
                (&mut w, &characters)
            } else {
                (&mut legacy, &legacy_characters)
            };
            let start = Instant::now();
            for &c in actors {
                world.step_character(c, input(Vec3::X))?;
            }
            world.step()?;
            if tree {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            } else {
                legacy_times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    let same_state =
        hash(&w, &ids, &characters, &[]) == hash(&legacy, &legacy_ids, &legacy_characters, &[]);
    if !same_state {
        return Err("spatial character broadphase differs from all-character reference".into());
    }
    let outpost = serde_json::json!({"scenario":"outpost","characters":1000,"props":2000,"threads":threads,"steps":steps,"preset":"game_default","includesCharacterMovement":true,"ms":distribution(&mut times),"legacyMs":distribution(&mut legacy_times),"identicalFinalState":same_state,"activeAtEnd":w.backend().active_count(),"loadBefore":load_before,"loadAfter":load()});
    let mut query_records = Vec::new();
    let sphere = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.2 })?;
    for casts in [false, true] {
        let load_before = load();
        let queries = (0..100000)
            .map(|i| {
                let origin = [
                    (i % 400) as f32 * 0.1 - 20.0,
                    25.0,
                    (i / 400) as f32 * 0.1 - 12.5,
                ];
                if casts {
                    Query::Cast {
                        shape: sphere,
                        pose: PhysicsPose {
                            position: origin,
                            ..Default::default()
                        },
                        displacement: [0.0, -30.0, 0.0],
                        filter: QueryFilter::default(),
                    }
                } else {
                    Query::Ray {
                        origin,
                        displacement: [0.0, -30.0, 0.0],
                        filter: QueryFilter::default(),
                    }
                }
            })
            .collect::<Vec<_>>();
        let mut results = QueryResults::default();
        w.backend().queries(&queries, &mut results)?;
        let mut times = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            w.backend().queries(&queries, &mut results)?;
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        query_records.push(serde_json::json!({"scenario":if casts {"100k_shape_casts"}else{"100k_rays"},"queries":100000,"samples":5,"hits":results.hits.len(),"semantics":"all hits, sorted, includes FFI conversion/allocation","ms":distribution(&mut times),"loadBefore":load_before,"loadAfter":load()}));
    }
    let mut states = Vec::new();
    w.backend_mut().read_bodies(&ids, &mut states)?;
    if !states
        .iter()
        .all(|s| s.pose.position.iter().all(|v| v.is_finite()))
    {
        return Err("nonfinite Outpost state".into());
    }
    query_records.insert(0, outpost);
    query_records.push(ecs(threads, steps)?);
    query_records.push(contacts(threads, steps)?);
    Ok(query_records)
}

fn contacts(threads: u32, steps: usize) -> BenchResult<serde_json::Value> {
    let load_before = load();
    let count = 5000;
    let c = config("game_default", count, threads)?;
    let (mut off, ids) = fixture(c.clone(), count)?;
    let mut subscribed = c;
    subscribed.event_capacity = count as u32 * 64;
    let (mut listener, listener_ids) = fixture(subscribed.clone(), count)?;
    let (mut decoded, decoded_ids) = fixture(subscribed, count)?;
    let mut times: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::with_capacity(steps));
    let mut effects = 0;
    for tick in 0..steps {
        for side in 0..3 {
            let mode = (side + tick) % 3;
            let start = Instant::now();
            match mode {
                0 => off.step()?,
                1 => listener.backend_mut().benchmark_native_step(DT)?,
                _ => decoded.step()?,
            }
            times[mode].push(start.elapsed().as_secs_f64() * 1000.0);
        }
        effects += decoded.events().contacts.len() + decoded.events().triggers.len();
    }
    let expected = hash(&off, &ids, &[], &[]);
    if hash(&listener, &listener_ids, &[], &[]) != expected
        || hash(&decoded, &decoded_ids, &[], &[]) != expected
    {
        return Err("contact subscription changed rigid-body state".into());
    }
    Ok(
        serde_json::json!({"scenario":"contact_subscription","bodies":count,"steps":steps,"threads":threads,
        "preset":"game_default","offMs":distribution(&mut times[0]),"listenerOnlyMs":distribution(&mut times[1]),
        "decodedMs":distribution(&mut times[2]),"effects":effects,"identicalFinalState":true,"loadBefore":load_before,"loadAfter":load()}),
    )
}

fn ecs(threads: u32, steps: usize) -> BenchResult<serde_json::Value> {
    use ara_physics_jolt::JoltBackend;
    use bevy_ecs::prelude::{Schedule, World};
    let load_before = load();
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register::<JoltBackend>(
        &mut world,
        &mut schedule,
        config("game_default", 20001, threads)?,
    )?;
    world.spawn((
        PhysicsId { value: 1 },
        PhysicsPose {
            position: [0.0, -0.5, 0.0],
            ..Default::default()
        },
        RigidBody::default(),
        Collider {
            half_extents: [100.0, 0.5, 100.0],
            ..Default::default()
        },
        Velocity::default(),
    ));
    for i in 0..20000 {
        world.spawn((
            PhysicsId { value: i + 2 },
            PhysicsPose {
                position: [
                    (i % 100) as f32 * 0.6 - 30.0,
                    2.0 + (i / 10000) as f32 * 0.6,
                    ((i / 100) % 100) as f32 * 0.6 - 30.0,
                ],
                ..Default::default()
            },
            RigidBody {
                motion: MotionType::Dynamic,
                ..Default::default()
            },
            Collider {
                half_extents: [0.25; 3],
                friction: 0.5,
                ..Default::default()
            },
            Velocity::default(),
        ));
    }
    let start = Instant::now();
    prepare_ecs::<JoltBackend>(&mut world)?;
    let setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut phases: [Vec<f64>; 6] = std::array::from_fn(|_| Vec::with_capacity(steps));
    let mut total = Vec::with_capacity(steps);
    for _ in 0..steps {
        let start = Instant::now();
        tick_ecs::<JoltBackend>(&mut world)?;
        let maintenance = Instant::now();
        world.clear_trackers();
        phases[5].push(maintenance.elapsed().as_secs_f64() * 1000.0);
        total.push(start.elapsed().as_secs_f64() * 1000.0);
        let t = world.resource::<PhysicsTimings>();
        for (times, value) in phases.iter_mut().zip([
            t.synchronize_ns,
            t.inputs_ns,
            t.step_ns,
            t.extraction_ns,
            t.writeback_ns,
        ]) {
            times.push(value as f64 / 1e6);
        }
    }
    Ok(
        serde_json::json!({"scenario":"ecs_flat_grid","bodies":20000,"threads":threads,"steps":steps,"setupMs":setup_ms,"totalIncludesTrackerMaintenance":true,"totalMs":distribution(&mut total),"phaseOrder":["lifecycle","inputs","step","extraction","writeback","tracker_maintenance"],"phaseMs":phases.iter_mut().map(|p|distribution(p)).collect::<Vec<_>>(),"loadBefore":load_before,"loadAfter":load()}),
    )
}
