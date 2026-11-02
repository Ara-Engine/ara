use super::*;
use crate as fixture_api;
use crate::look::transport::{
    links::{Candidate, LinkStore, Visibility},
    patches::{PatchRegistry, Placement},
    reverse::ReverseLinks,
};
use anyhow::Result;
use ara_patch_charts::{Mesh, Settings, Triangle, cache};
use std::sync::Arc;
#[path = "bench/fixtures.rs"]
mod fixtures;

#[test]
fn native_local_repair_matches_fresh_trace_through_move_sleep_and_wake() -> Result<()> {
    use crate::instanced::{
        Changes,
        transport::{BodyClass, NativeChangeFeed},
    };
    use glam::Mat4;
    let case = fixtures::Case {
        name: "sleep-wake",
        frames: 90,
        event: 8,
        bodies: 1,
        triangles: 108,
        lights: 1,
    };
    let mut f = fixtures::create(&case)?;
    f.scene
        .instances
        .retain(|i| i.entity == 1 || i.entity == 100);
    f.scene.instances[0].world =
        Mat4::from_translation(Vec3::new(0., -0.1, 0.)) * Mat4::from_scale(Vec3::new(4., 0.2, 4.));
    f.scene.instances[1].world =
        Mat4::from_translation(Vec3::new(-1., 0.6, 0.)) * Mat4::from_scale(Vec3::splat(0.8));
    f.scene.lights[0].position_range = [-3., 1., 0., 8.];
    f.scene.lights[0].parameters[2] = 0.;
    f.scene.sun_lux = Vec3::ZERO;
    f.scene.environment = vec![[0., 0., 0., 1.]];
    // An absorbing mover isolates source-shadow support from the receiver
    // volume approximation while testing the actual native event adapter.
    f.scene.instances[1].material = 1;
    f.scene.materials[1].pbr.base_color = [0., 0., 0., 1.];
    f.bodies.clear();
    f.bodies.insert(1, BodyClass::PhysicsActive);
    let mut transport = PatchTransport::new(
        &f.scene,
        &[],
        f.bodies.clone(),
        TransportOptions::default(),
        None,
    )?;
    let awake_count = transport.graph.links.patches.len();
    assert_eq!(transport.graph.repaired_index_bytes(), 0);
    assert!(
        transport
            .volume
            .sample(Vec3::new(-1., 0.6, 0.), Vec3::Y)
            .is_some()
    );
    let mut feed = NativeChangeFeed::new(&f.scene, 0)?;
    let mut changes = Changes::default();
    changes.transport.bodies.push((1, BodyClass::PhysicsActive));
    feed.update(&f.scene, Some(&changes))?;
    feed.finish();
    for (frame, class) in [
        (1, BodyClass::PhysicsActive),
        (2, BodyClass::Sleeping),
        (3, BodyClass::PhysicsActive),
    ] {
        f.scene.instances[1].world.w_axis.x = 0.5;
        changes.instances = vec![1];
        changes.transport.bodies = vec![(1, class)];
        f.bodies.insert(1, class);
        feed.begin_frame(frame)?;
        feed.update(&f.scene, Some(&changes))?;
        transport.advance(&f.scene, &[], feed.finish(), true)?;
        let fresh = PatchTransport::new(
            &f.scene,
            &[],
            f.bodies.clone(),
            TransportOptions::default(),
            None,
        )?;
        assert_eq!(
            transport.graph.links.source, fresh.graph.links.source,
            "frame {frame}"
        );
        assert_eq!(
            transport.graph.links.transmission, fresh.graph.links.transmission,
            "frame {frame}"
        );
        for (a, b) in transport.solver.forcing.iter().zip(&fresh.solver.forcing) {
            assert!(
                magnitude(*a - *b) < 1e-4,
                "source shadow mismatch at frame {frame}: {a:?} != {b:?}"
            );
        }
        if class == BodyClass::Sleeping {
            assert!(transport.graph.links.patches.len() > awake_count);
        } else {
            assert_eq!(transport.graph.links.patches.len(), awake_count);
        }
    }
    Ok(())
}

#[test]
fn media_motion_reuses_static_witnesses_but_material_edits_advance_them() -> Result<()> {
    use crate::instanced::{
        Changes,
        transport::{BodyClass, NativeChangeFeed},
    };
    use glam::Mat4;
    let case = fixtures::Case {
        name: "sleep-wake",
        frames: 90,
        event: 8,
        bodies: 1,
        triangles: 108,
        lights: 1,
    };
    let mut f = fixtures::create(&case)?;
    f.scene
        .instances
        .retain(|i| matches!(i.entity, 1 | 2 | 100));
    f.scene.instances[0].world =
        Mat4::from_translation(Vec3::new(0., -0.1, 0.)) * Mat4::from_scale(Vec3::new(4., 0.2, 4.));
    f.scene.instances[1].world =
        Mat4::from_translation(Vec3::new(0., 1., -1.)) * Mat4::from_scale(Vec3::new(2., 2., 0.2));
    f.scene.instances[2].world =
        Mat4::from_translation(Vec3::new(-0.5, 0.4, 0.)) * Mat4::from_scale(Vec3::splat(0.25));
    f.bodies.clear();
    f.bodies.insert(2, BodyClass::PhysicsActive);
    let mut transport = PatchTransport::new(
        &f.scene,
        &[],
        f.bodies.clone(),
        TransportOptions::default(),
        None,
    )?;
    assert_eq!(transport.graph.repaired_index_bytes(), 0);
    let mut feed = NativeChangeFeed::new(&f.scene, 0)?;
    let mut changes = Changes::default();
    changes.transport.bodies.push((2, BodyClass::PhysicsActive));
    feed.update(&f.scene, Some(&changes))?;
    feed.finish();
    for frame in 1..=3 {
        f.scene.instances[2].world.w_axis.x += 0.1;
        changes.instances = vec![2];
        feed.begin_frame(frame)?;
        feed.update(&f.scene, Some(&changes))?;
        transport.advance(&f.scene, &[], feed.finish(), true)?;
        assert_eq!(
            transport.graph.repaired_index_bytes(),
            0,
            "static visibility was duplicated at frame {frame}"
        );
    }
    changes.instances.clear();
    changes.transport.bodies.clear();
    changes.materials = vec![2];
    f.scene.materials[2].pbr.base_color = [0.2, 0.3, 0.4, 1.];
    feed.begin_frame(4)?;
    feed.update(&f.scene, Some(&changes))?;
    transport.advance(&f.scene, &[], feed.finish(), true)?;
    assert!(transport.graph.repaired_index_bytes() > 0);
    assert!(transport.report().index_bytes_per_link < 8.);
    Ok(())
}

pub(super) fn two_patches() -> Result<LinkStore> {
    let mesh = Mesh {
        positions: &[[0., 0., 0.], [0.5, 0., 0.], [0.5, 0.25, 0.], [0., 0.25, 0.]],
        triangles: &[
            Triangle {
                vertices: [0, 1, 2],
                material: 0,
                albedo: [0.94; 3],
            },
            Triangle {
                vertices: [0, 2, 3],
                material: 0,
                albedo: [0.94; 3],
            },
        ],
    };
    let prepared = cache::prepare(mesh, Settings::default(), None)?;
    let mut registry = PatchRegistry::default();
    registry
        .publish(
            1,
            prepared.key,
            Arc::new(prepared.charts),
            0,
            Placement::default(),
        )
        .unwrap();
    let patches = Arc::new(registry.snapshot().unwrap());
    assert_eq!(patches.len(), 2);
    Ok(LinkStore::build(patches, vec![], 2, |receiver| {
        Ok(vec![Candidate {
            ordinal: 0,
            source: 1 - receiver,
            weight: 1.,
            visibility: Visibility::clear(),
            segment: None,
        }])
    })?)
}

#[test]
fn event_solver_high_albedo_switch_off_matches_independent_exact_solution() -> Result<()> {
    let links = two_patches()?;
    let reverse = ReverseLinks::build(&links)?;
    let mut solver = EventSolver::new(&links)?;
    solver.set_forcing(0, Vec3::new(1., 0.3, 0.1))?;
    let exact = solver.exact(&links, 1e-10)?;
    let stats = solver.run(&links, &reverse, Budget::default())?;
    assert!(stats.solution_bound <= 1e-5);
    for (value, reference) in solver.radiance.iter().zip(&exact) {
        assert!(magnitude(*value - *reference) < 3e-5);
    }
    solver.begin_frame(1)?;
    solver.set_forcing(0, Vec3::ZERO)?;
    solver.run(&links, &reverse, Budget::default())?;
    assert!(solver.radiance.iter().all(|v| magnitude(*v) < 2e-5));
    Ok(())
}

#[test]
fn white_receiver_converges_through_absorbing_dependency_without_albedo_clamp() -> Result<()> {
    let links = two_patches()?;
    let reverse = ReverseLinks::build(&links)?;
    let mut solver = EventSolver::new_albedo(&links, vec![Vec3::ONE, Vec3::splat(0.5)])?;
    solver.set_forcing(0, Vec3::ONE)?;
    solver.run(&links, &reverse, Budget::default())?;
    assert!(magnitude(solver.radiance[0] - Vec3::splat(2.)) < 2e-5);
    assert!(magnitude(solver.radiance[1] - Vec3::ONE) < 2e-5);
    assert!(EventSolver::new_albedo(&links, vec![Vec3::ONE; 2]).is_err());
    Ok(())
}

#[test]
fn repeated_events_keep_old_debt_and_reverse_propagation_cannot_be_cut() -> Result<()> {
    let links = two_patches()?;
    let reverse = ReverseLinks::build(&links)?;
    let mut solver = EventSolver::new(&links)?;
    for frame in 0..300 {
        solver.begin_frame(frame)?;
        solver.set_forcing(0, Vec3::splat(if frame % 2 == 0 { 1. } else { 0.2 }))?;
        let stats = solver.run(
            &links,
            &reverse,
            Budget {
                patch_passes: 1,
                reverse_edges: 1,
                solution_tolerance: 1e-8,
            },
        )?;
        assert_eq!(stats.patch_passes, 1);
        assert_eq!(stats.oldest_age, frame);
        if frame >= 3 {
            assert_eq!(stats.missed_deadlines, 1);
        }
    }
    let before = solver.radiance.to_vec();
    let stats = solver.run(
        &links,
        &reverse,
        Budget {
            reverse_edges: 0,
            ..Budget::default()
        },
    )?;
    assert_eq!(stats.patch_passes, 0);
    assert_eq!(solver.radiance, before);
    Ok(())
}

#[test]
fn channel_support_memory_and_generation_validity_are_explicit() -> Result<()> {
    use channels::{Channel, Channels, Domain};
    let links = two_patches()?;
    let reverse = ReverseLinks::build(&links)?;
    assert!(Channels::support(&links, &reverse, &[0], 1).is_none());
    assert_eq!(
        Channels::support(&links, &reverse, &[0], 2),
        Some(vec![0, 1])
    );
    let channel = || Channel {
        light: 7,
        shape_generation: 1,
        support: vec![(Domain::Material(3), 10)],
        values: vec![(0, Vec3::ONE), (1, Vec3::splat(0.5))],
        forcing: vec![],
        irradiance: vec![],
        valid: true,
        truncated: false,
        error_bound: 1e-7,
    };
    assert!(!Channels::new(1).insert(channel(), Vec3::ONE)?);
    let mut cache = Channels::new(4096);
    assert!(cache.insert(channel(), Vec3::ONE)?);
    cache.set_gain(7, Vec3::new(3., 2., 1.))?;
    assert_eq!(cache.read(1).unwrap().0, Vec3::new(1.5, 1., 0.5));
    cache.validate_generations(|_| Some(11));
    assert!(cache.read(0).is_none());
    Ok(())
}

#[test]
fn direction_patterns_are_deterministic_normalized_and_distinct() {
    let n = Vec3::new(0.3, 0.7, 0.2).normalize();
    for scheme in [DirectionScheme::Global, DirectionScheme::PerPatch] {
        let a = graph::directions(n, 8, 48, scheme);
        let b = graph::directions(n, 8, 48, scheme);
        assert_eq!(a, b);
        assert!((a.iter().map(|v| v.1).sum::<f32>() - 1.).abs() < 1e-6);
        assert!(
            a.iter()
                .all(|(d, w)| d.dot(n) > 0. && (d.length() - 1.).abs() < 1e-5 && *w > 0.)
        );
    }
    assert_ne!(
        graph::directions(n, 8, 48, DirectionScheme::Global),
        graph::directions(n, 8, 48, DirectionScheme::PerPatch)
    );
}
