use super::*;
use glam::Quat;

struct CountedAllocator;
thread_local! { static ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;
// SAFETY: all allocation and deallocation requests are forwarded unchanged to System.
unsafe impl std::alloc::GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        // SAFETY: the caller supplies the allocation layout required by GlobalAlloc.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: ptr/layout are the unchanged pair supplied by the allocator caller.
        unsafe {
            std::alloc::System.dealloc(ptr, layout);
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        // SAFETY: the original pointer/layout and new size are forwarded unchanged.
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
}

fn instance() -> InstanceState {
    InstanceState {
        entity: 91,
        geometry: 0,
        material: 0,
        world: Mat4::IDENTITY,
        bounds: Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5)),
        body: BodyClass::Static,
        casts_shadows: true,
        wind: 0.,
    }
}
#[test]
fn generations_survive_removal_reuse_and_publication_reset() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    feed.begin_frame(0)?;
    feed.set_instance(4, instance(), 0., 0)?;
    assert_eq!(feed.instance(4).unwrap().generation, 1);
    feed.finish();
    feed.begin_frame(1)?;
    feed.set_instance(4, instance(), 0., 0)?;
    assert!(feed.finish().instances.is_empty());
    feed.begin_frame(2)?;
    feed.remove_instance(4)?;
    assert_eq!(feed.finish().instances[0].change.generation, 2);
    feed.begin_frame(3)?;
    feed.set_instance(4, instance(), 0., 0)?;
    assert_eq!(feed.instance(4).unwrap().generation, 3);
    feed.finish();
    feed.begin_frame(4)?;
    feed.reset()?;
    feed.set_instance(4, instance(), 0., 0)?;
    assert_eq!(feed.instance(4).unwrap().generation, 4);
    assert!(feed.finish().reset);
    Ok(())
}

#[test]
fn all_cache_stamps_are_monotone_and_overflow_is_rejected() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    populate(&mut feed, 0)?;
    feed.finish();
    feed.begin_frame(1)?;
    feed.set_geometry(0, None)?;
    feed.set_material(0, None)?;
    feed.set_light(0, None)?;
    feed.set_portal(5, None)?;
    feed.set_sun(None)?;
    let removed = feed.finish();
    assert_eq!(removed.geometries[0].generation, 2);
    assert_eq!(removed.materials[0].generation, 2);
    assert_eq!(removed.lights[0].generation, 2);
    assert_eq!(removed.portals[0].generation, 2);
    assert_eq!(removed.sun.unwrap().change.generation, 2);
    populate(&mut feed, 2)?;
    feed.finish();
    assert_eq!(feed.geometry(0).unwrap().generation, 3);
    assert_eq!(feed.material(0).unwrap().generation, 3);
    assert_eq!(feed.light(0).unwrap().generation, 3);
    assert_eq!(feed.portal(5).unwrap().generation, 3);
    assert_eq!(feed.sun().unwrap().generation, 3);
    let mut snapshot = TransportChanges {
        frame: 0,
        reset: true,
        ..Default::default()
    };
    snapshot.instances.push(InstanceChange {
        change: Change {
            id: 0,
            previous_generation: 0,
            generation: u64::MAX,
            flags: flags::ADDED,
            previous: None,
            current: Some(instance()),
        },
        swept: instance().world_bounds(),
        displacement: 0.,
    });
    let mut exhausted = ChangeFeed::default();
    exhausted.apply_replay(&snapshot)?;
    exhausted.begin_frame(1)?;
    let mut next = instance();
    next.world.w_axis.x = 1.;
    assert!(exhausted.set_instance(0, next, 0., 0).is_err());
    assert_eq!(exhausted.instance(0).unwrap().value, instance());
    Ok(())
}
#[test]
fn sleep_wake_and_motion_are_not_lost_when_coalesced() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    let mut state = instance();
    state.body = BodyClass::Sleeping;
    feed.begin_frame(0)?;
    feed.set_instance(1, state, 0., 0)?;
    feed.finish();
    feed.begin_frame(1)?;
    state.body = BodyClass::PhysicsActive;
    state.world = Mat4::from_translation(Vec3::X * 5.);
    feed.set_instance(1, state, 0., 0)?;
    state.body = BodyClass::Sleeping;
    state.world = Mat4::IDENTITY;
    feed.set_instance(1, state, 0., 0)?;
    let row = &feed.finish().instances[0];
    assert_ne!(row.change.flags & flags::SLEEP, 0);
    assert_ne!(row.change.flags & flags::WAKE, 0);
    assert!(row.swept.max.x >= 5.5);
    assert!(row.displacement >= 10.);
    assert_eq!(row.change.generation, 3);
    assert!(BodyClass::Sleeping.owns_links());
    assert!(!BodyClass::PhysicsActive.owns_links());
    Ok(())
}
#[test]
fn random_transforms_bound_corners_and_rotation_interiors() {
    let mut seed = 12345u64;
    let mut random = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 32) as u32) as f32 / u32::MAX as f32
    };
    for _ in 0..500 {
        let a_scale = Vec3::new(
            0.1 + random() * 3.,
            0.1 + random() * 3.,
            -(0.1 + random() * 3.),
        );
        let b_scale = Vec3::new(
            0.1 + random() * 3.,
            0.1 + random() * 3.,
            -(0.1 + random() * 3.),
        );
        let a_rot = Quat::from_euler(
            glam::EulerRot::XYZ,
            random() * 6.,
            random() * 6.,
            random() * 6.,
        );
        let b_rot = Quat::from_euler(
            glam::EulerRot::XYZ,
            random() * 6.,
            random() * 6.,
            random() * 6.,
        );
        let a_pos = Vec3::new(random() * 50., random() * 50., random() * 50.);
        let b_pos = Vec3::new(random() * 50., random() * 50., random() * 50.);
        let mut a = instance();
        a.bounds = Aabb::new(Vec3::new(-2., -0.1, 0.2), Vec3::new(0.2, 0.3, 1.5));
        a.world = Mat4::from_scale_rotation_translation(a_scale, a_rot, a_pos);
        let mut b = a;
        b.world = Mat4::from_scale_rotation_translation(b_scale, b_rot, b_pos);
        let motion = motion_bounds(a, b, 0.);
        for p in a.bounds.corners() {
            assert!(
                (b.world.transform_point3(p) - a.world.transform_point3(p)).length()
                    <= motion.displacement
            );
            for sample in 0..=16 {
                let t = sample as f32 / 16.;
                let m = Mat4::from_scale_rotation_translation(
                    a_scale.lerp(b_scale, t),
                    a_rot.slerp(b_rot, t),
                    a_pos.lerp(b_pos, t),
                );
                let point = m.transform_point3(p);
                assert!(point.cmpge(motion.swept.min).all() && point.cmple(motion.swept.max).all());
            }
        }
    }
}
#[test]
fn rotation_translation_deformation_wind_and_size() {
    let a = instance();
    let mut b = a;
    b.world =
        Mat4::from_rotation_translation(Quat::from_rotation_y(std::f32::consts::PI), Vec3::X * 3.);
    let motion = motion_bounds(a, b, 0.);
    assert!(motion.displacement >= 17f32.sqrt());
    b.wind = 0.2;
    let deform = motion_bounds(a, b, 0.5);
    assert!(deform.displacement > motion.displacement + 0.7);
    b.world = Mat4::from_scale(Vec3::splat(0.499));
    b.wind = 0.;
    assert!(!b.is_large());
    b.world = Mat4::from_scale(Vec3::splat(0.5));
    assert!(b.is_large());
}
fn populate(feed: &mut ChangeFeed, frame: u64) -> anyhow::Result<()> {
    feed.begin_frame(frame)?;
    let mut s = instance();
    s.world = Mat4::from_translation(Vec3::X * frame as f32);
    feed.set_instance(7, s, 0., 0)?;
    feed.set_instance(2, s, 0., flags::PAINT)?;
    feed.set_geometry(
        0,
        Some(GeometryState {
            key: [9; 32],
            revision: frame,
            bounds: s.bounds,
            displacement: 0.1,
        }),
    )?;
    let mut material = crate::instanced::Material::default();
    material.pbr.base_color[0] = frame as f32;
    feed.set_material(0, Some(MaterialState::from(&material)))?;
    let mut light = crate::scene::Light::ZERO;
    light.color_lumens[3] = frame as f32;
    feed.set_light(0, Some(LightState::from(&light)))?;
    feed.set_portal(
        5,
        Some(PortalState {
            openness: (frame % 2) as f32,
        }),
    )?;
    feed.set_sun(Some(SunState {
        direction: Vec3::new(1., 0.1 + frame as f32, 0.).normalize(),
        lux: Vec3::splat(100.),
    }))?;
    Ok(())
}
#[test]
fn replay_round_trip_is_identical_including_cache_state() -> anyhow::Result<()> {
    let mut source = ChangeFeed::default();
    let mut writer = ReplayWriter::new(Vec::new())?;
    let mut expected = Vec::new();
    for frame in 0..5 {
        populate(&mut source, frame)?;
        expected.push(source.finish().clone());
        writer.write_frame(source.changes())?;
    }
    let bytes = writer.finish()?;
    assert_eq!(&bytes[..16], b"ARACHG01\x01\0\0\0\0\0\0\0");
    let mut reader = ReplayReader::new(bytes.as_slice(), 1 << 20)?;
    let mut actual = TransportChanges::default();
    let mut replay = ChangeFeed::default();
    for frame in expected {
        assert!(reader.read_frame(&mut actual)?);
        assert_eq!(actual, frame);
        replay.apply_replay(&actual)?;
        assert_eq!(replay.changes(), &frame);
    }
    assert!(!reader.read_frame(&mut actual)?);
    assert_eq!(replay.instance(7), source.instance(7));
    assert_eq!(replay.geometry(0), source.geometry(0));
    assert_eq!(replay.light(0), source.light(0));
    Ok(())
}

#[test]
fn warmed_replay_and_recording_allocate_nothing() -> anyhow::Result<()> {
    let mut source = ChangeFeed::default();
    let mut writer = ReplayWriter::new(Vec::new())?;
    for frame in 0..12 {
        populate(&mut source, frame)?;
        writer.write_frame(source.finish())?;
    }
    let bytes = writer.finish()?;
    let mut reader = ReplayReader::new(bytes.as_slice(), 1 << 20)?;
    let mut replay = ChangeFeed::default();
    let mut out = TransportChanges::default();
    let mut sink = ReplayWriter::new(std::io::sink())?;
    for _ in 0..3 {
        assert!(reader.read_frame(&mut out)?);
        replay.apply_replay(&out)?;
        sink.write_frame(&out)?;
    }
    ALLOCATIONS.with(|n| n.set(Some(0)));
    for _ in 3..12 {
        assert!(reader.read_frame(&mut out)?);
        replay.apply_replay(&out)?;
        sink.write_frame(&out)?;
    }
    let allocations = ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(allocations, 0);
    Ok(())
}
#[test]
fn replay_rejects_truncation_limits_bad_order_and_wrong_predecessor() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    populate(&mut feed, 0)?;
    let initial = feed.finish().clone();
    let mut writer = ReplayWriter::new(Vec::new())?;
    writer.write_frame(&initial)?;
    let bytes = writer.finish()?;
    for len in 17..bytes.len() {
        let mut reader = ReplayReader::new(&bytes[..len], 1 << 20)?;
        assert!(reader.read_frame(&mut TransportChanges::default()).is_err());
    }
    let mut limited = ReplayReader::new(bytes.as_slice(), 22)?;
    assert!(
        limited
            .read_frame(&mut TransportChanges::default())
            .is_err()
    );
    let mut bad = initial.clone();
    bad.instances.reverse();
    assert!(ReplayWriter::new(Vec::new())?.write_frame(&bad).is_err());
    let mut replay = ChangeFeed::default();
    replay.apply_replay(&initial)?;
    populate(&mut feed, 1)?;
    let mut next = feed.finish().clone();
    next.instances[0].change.previous_generation += 1;
    assert!(replay.apply_replay(&next).is_err());
    assert_eq!(replay.changes(), &initial);
    Ok(())
}
#[test]
fn flicker_does_not_report_visibility_change_and_low_sun_is_explicit() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    populate(&mut feed, 0)?;
    feed.finish();
    populate(&mut feed, 1)?;
    let changes = feed.finish();
    assert_eq!(changes.lights[0].flags, flags::INTENSITY);
    assert!(changes.sun.unwrap().angular_delta > 0.);
    assert!(changes.sun.unwrap().elevation > 0.);
    Ok(())
}
#[test]
fn warmed_frame_storage_is_reused() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    populate(&mut feed, 0)?;
    feed.finish();
    populate(&mut feed, 1)?;
    let first = feed.finish();
    let pointers = (
        first.instances.as_ptr(),
        first.geometries.as_ptr(),
        first.materials.as_ptr(),
        first.lights.as_ptr(),
        first.portals.as_ptr(),
    );
    for frame in 2..100 {
        populate(&mut feed, frame)?;
        let rows = feed.finish();
        assert_eq!(
            pointers,
            (
                rows.instances.as_ptr(),
                rows.geometries.as_ptr(),
                rows.materials.as_ptr(),
                rows.lights.as_ptr(),
                rows.portals.as_ptr()
            )
        );
    }
    Ok(())
}

#[test]
fn steady_state_allocates_zero_for_four_thousand_movers() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    for frame in 0..2 {
        feed.begin_frame(frame)?;
        for id in 0..4000 {
            let mut s = instance();
            s.entity = id;
            s.world = Mat4::from_translation(Vec3::X * frame as f32);
            feed.set_instance(id, s, 0., 0)?;
        }
        feed.finish();
    }
    ALLOCATIONS.with(|n| n.set(Some(0)));
    for frame in 2..12 {
        feed.begin_frame(frame)?;
        for id in 0..4000 {
            let mut s = instance();
            s.entity = id;
            s.world = Mat4::from_translation(Vec3::X * frame as f32);
            feed.set_instance(id, s, 0., 0)?;
        }
        std::hint::black_box(feed.finish());
    }
    let allocations = ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(
        allocations, 0,
        "warmed updates must reuse maps and event storage"
    );
    Ok(())
}

#[test]
#[ignore = "CPU timing probe; run in a quiet window for comparable numbers"]
fn measure_change_feed_four_thousand_movers() -> anyhow::Result<()> {
    let mut feed = ChangeFeed::default();
    let mut times = Vec::with_capacity(300);
    for frame in 0..302 {
        let start = std::time::Instant::now();
        feed.begin_frame(frame)?;
        for id in 0..4000 {
            let mut s = instance();
            s.entity = id;
            s.body = BodyClass::PhysicsActive;
            s.world = Mat4::from_rotation_translation(
                Quat::from_rotation_y(frame as f32 * 0.01),
                Vec3::new(id as f32 * 0.6, frame as f32 * 0.001, 0.),
            );
            feed.set_instance(id, s, 0., 0)?;
        }
        std::hint::black_box(feed.finish());
        if frame >= 2 {
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    times.sort_by(f64::total_cmp);
    let mut recording = ReplayWriter::new(Vec::new())?;
    let mut frame = feed.changes().clone();
    frame.reset = true;
    for c in &mut frame.instances {
        c.change.previous = None;
        c.change.flags |= flags::ADDED;
    }
    recording.write_frame(&frame)?;
    let bytes = recording.finish()?.len();
    println!(
        "A3 4000 rotating movers/300 frames: p50={:.6} ms p99={:.6} ms max={:.6} ms; full snapshot={} bytes; InstanceChange={} bytes",
        times[150],
        times[296],
        times[299],
        bytes,
        std::mem::size_of::<InstanceChange>()
    );
    Ok(())
}
