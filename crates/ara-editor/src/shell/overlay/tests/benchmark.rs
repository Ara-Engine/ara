//! Opt-in differential timing of the original and validated translation hit paths.
use super::*;
use std::{hint::black_box, time::Instant};

// Keep the pre-fix classifier only in this benchmark so the parent test stream can
// measure the same projected inputs before/after without rebuilding two editors.
fn legacy_quad(handles: &Handles, axis: usize) -> Option<[[f32; 2]; 4]> {
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let o = Vec2::from(handles.origin);
    if (Vec2::from(handles.ends[a]) - o).length() < 20.
        || (Vec2::from(handles.ends[b]) - o).length() < 20.
    {
        return None;
    }
    let (u, v) = (
        handles.axes[a] * handles.length,
        handles.axes[b] * handles.length,
    );
    let corner =
        |s: f32, t: f32| project(&handles.camera, handles.rect, handles.pivot + u * s + v * t);
    Some([
        corner(0.2, 0.2)?,
        corner(0.4, 0.2)?,
        corner(0.4, 0.4)?,
        corner(0.2, 0.4)?,
    ])
}

fn legacy_inside(quad: [[f32; 2]; 4], point: Vec2) -> bool {
    let corners = quad.map(Vec2::from);
    let area: f32 = (0..4)
        .map(|n| corners[n].perp_dot(corners[(n + 1) % 4]))
        .sum();
    (0..4).all(|n| {
        let (a, b) = (corners[n], corners[(n + 1) % 4]);
        let edge = b - a;
        edge.length() < 0.001 || area.signum() * edge.perp_dot(point - a) / edge.length() > -2.
    })
}

fn legacy_hit(handles: &Handles, pointer: [f32; 2]) -> Option<Handle> {
    let (p, o) = (Vec2::from(pointer), Vec2::from(handles.origin));
    if p.distance(o) < HIT {
        return Some(Handle::Centre);
    }
    for axis in 0..3 {
        if legacy_quad(handles, axis).is_some_and(|quad| legacy_inside(quad, p)) {
            return Some(Handle::Plane(axis));
        }
    }
    (0..3)
        .filter_map(|axis| {
            let ab = Vec2::from(handles.ends[axis]) - o;
            if ab.length() < 4. {
                return None;
            }
            let t = ((p - o).dot(ab) / ab.length_squared()).clamp(0.15, 1.15);
            let distance = p.distance(o + ab * t);
            (distance < HIT).then_some((Handle::Axis(axis), distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle)
}

const ITERATIONS: usize = 500;
fn measure(
    classify: fn(&Handles, [f32; 2]) -> Option<Handle>,
    inputs: &[(Handles, [f32; 2])],
) -> (f64, usize) {
    let started = Instant::now();
    let mut hits = 0;
    for _ in 0..ITERATIONS {
        for (handles, pointer) in inputs {
            hits +=
                usize::from(black_box(classify(black_box(handles), black_box(*pointer))).is_some());
        }
    }
    (started.elapsed().as_secs_f64() * 1000., hits)
}

#[test]
#[ignore = "explicit before/after gizmo hit CPU benchmark; writes only target evidence"]
fn gizmo_hit_before_after_benchmark() -> ara_ui::Result<()> {
    let mut inputs = Vec::new();
    for projection in [
        CameraProjection::Perspective,
        CameraProjection::Orthographic,
    ] {
        for yaw in [0., 0.6, std::f32::consts::FRAC_PI_2, std::f32::consts::PI] {
            let handles = fixture(
                camera(projection, yaw, if yaw == 0.6 { 0.4 } else { 0. }),
                Vec3::new(-4., 0., 0.),
                Tool::Translate,
            );
            inputs.push((handles, handles.origin));
            for axis in 0..3 {
                inputs.push((handles, axis_point(&handles, axis)));
                if let Some(quad) = handles.plane_quad(axis) {
                    inputs.push((handles, centroid(quad)));
                }
            }
            inputs.push((handles, [handles.origin[0] + 10_000., handles.origin[1]]));
            inputs.push((
                handles,
                [handles.origin[0] + 120., handles.origin[1] + 120.],
            ));
        }
    }
    // Warm both paths, then alternate measurement order to reduce cache/order bias.
    black_box(measure(legacy_hit, &inputs));
    black_box(measure(hit, &inputs));
    let (mut before, mut after) = (Vec::new(), Vec::new());
    let (mut before_hits, mut after_hits) = (0, 0);
    for sample in 0..6 {
        let ((before_ms, before_count), (after_ms, after_count)) = if sample % 2 == 0 {
            (measure(legacy_hit, &inputs), measure(hit, &inputs))
        } else {
            let after = measure(hit, &inputs);
            (measure(legacy_hit, &inputs), after)
        };
        before.push(before_ms);
        after.push(after_ms);
        before_hits = before_count;
        after_hits = after_count;
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    let native = fixture(
        camera(CameraProjection::Perspective, 0., 0.),
        Vec3::new(-4., 0., 0.),
        Tool::Translate,
    );
    let pointer = axis_point(&native, 0);
    assert_eq!(
        legacy_hit(&native, pointer),
        Some(Handle::Plane(1)),
        "baseline must reproduce the original failure"
    );
    assert_eq!(hit(&native, pointer), Some(Handle::Axis(0)));
    let queries = inputs.len() * ITERATIONS;
    let report = json!({
        "tool":"translate", "inputs":inputs.len(), "iterations":ITERATIONS, "queriesPerSample":queries,
        "beforeSamplesMs":before, "afterSamplesMs":after,
        "beforeMedianNsPerHit":before[before.len()/2] * 1_000_000. / queries as f64,
        "afterMedianNsPerHit":after[after.len()/2] * 1_000_000. / queries as f64,
        "beforeHitCount":before_hits, "afterHitCount":after_hits,
        "nativeFailure":{"pivot":native.pivot.to_array(),"origin":native.origin,"pointer":pointer,
            "before":format!("{:?}",legacy_hit(&native,pointer)),"after":format!("{:?}",hit(&native,pointer))}
    });
    let output = crate::test_target().join("ux-p4/gizmo-hit-benchmark.json");
    std::fs::create_dir_all(output.parent().ok_or("benchmark output has no parent")?)?;
    std::fs::write(&output, ara_json::to_canonical_string(&report))?;
    println!("{}", ara_json::to_canonical_string(&report));
    Ok(())
}
