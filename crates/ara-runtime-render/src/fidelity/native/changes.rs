//! Producer checks: bounds are measured from the vertices actually uploaded.
use super::*;

#[test]
fn skin_displacement_covers_rotation_translation_and_preserves_vertex_bytes() {
    let source = vec![
        ModelVertex {
            position: [1., 0., 0.],
            normal: [0., 1., 0.],
            tangent: [1., 0., 0., 1.],
            weights: [1., 0., 0., 0., 0., 0., 0., 0.],
            ..Default::default()
        },
        ModelVertex {
            position: [-1., 0., 0.],
            normal: [0., 1., 0.],
            tangent: [1., 0., 0., 1.],
            weights: [1., 0., 0., 0., 0., 0., 0., 0.],
            ..Default::default()
        },
    ];
    let mut geometry = render::Geometry {
        key: [2; 32],
        vertices: source
            .iter()
            .map(|v| render::Vertex {
                position: v.position,
                normal: v.normal,
                tangent: v.tangent,
                uv: v.uv,
                uv1: v.uv1,
                terrain_weights: 0,
                color: u32::MAX,
            })
            .collect(),
        bounds: [glam30::Vec3::NEG_X, glam30::Vec3::X],
        lods: Vec::new(),
        deforming: true,
        revision: 0,
        vertex_colors: false,
    };
    let palette = [glam::Mat4::from_rotation_translation(
        glam::Quat::from_rotation_y(std::f32::consts::PI),
        glam::Vec3::X * 3.,
    )];
    let mut expected = Vec::new();
    crate::geometry::transform(&source, glam::Mat4::IDENTITY, Some(&palette), &mut expected);
    let mut scratch = Vec::new();
    let bound = update_skin(&mut geometry, &source, &palette, &mut scratch);
    assert!((5. ..5.001).contains(&bound));
    assert_eq!(geometry.revision, 1);
    for (v, expected) in geometry.vertices.iter().zip(expected) {
        assert_eq!(v.position, expected.position);
        assert_eq!(v.normal, expected.normal);
        assert_eq!(v.tangent, expected.tangent);
    }
    assert_eq!(
        update_skin(&mut geometry, &source, &palette, &mut scratch),
        0.
    );
}

#[test]
fn transport_hints_are_reused_and_cleared_with_legacy_changes() {
    let mut changes = render::Changes::default();
    changes.instances.push(3);
    changes
        .transport
        .bodies
        .push((3, render::transport::BodyClass::Sleeping));
    changes.transport.deformations.push((0, 2.));
    let capacity = changes.transport.deformations.capacity();
    changes.clear();
    assert!(changes.instances.is_empty());
    assert!(changes.transport.bodies.is_empty());
    assert!(changes.transport.deformations.is_empty());
    assert_eq!(changes.transport.deformations.capacity(), capacity);
}

#[test]
#[ignore = "CPU before/after skin producer timing; use a quiet window"]
fn measure_skin_displacement_overhead() {
    let source: Vec<_> = (0..4096)
        .map(|i| ModelVertex {
            position: [(i % 64) as f32 * 0.02, (i / 64) as f32 * 0.02, 0.],
            normal: [0., 0., 1.],
            tangent: [1., 0., 0., 1.],
            weights: [1., 0., 0., 0., 0., 0., 0., 0.],
            ..Default::default()
        })
        .collect();
    let make = || render::Geometry {
        key: [0; 32],
        vertices: source
            .iter()
            .map(|v| render::Vertex {
                position: v.position,
                normal: v.normal,
                tangent: v.tangent,
                uv: v.uv,
                uv1: v.uv1,
                terrain_weights: 0,
                color: u32::MAX,
            })
            .collect(),
        bounds: [glam30::Vec3::ZERO; 2],
        lods: vec![],
        deforming: true,
        revision: 0,
        vertex_colors: false,
    };
    let mut old = make();
    let mut new = make();
    let mut old_scratch = Vec::with_capacity(source.len());
    let mut new_scratch = Vec::with_capacity(source.len());
    let mut old_times = Vec::with_capacity(300);
    let mut new_times = Vec::with_capacity(300);
    for frame in 0..302 {
        let palette = [glam::Mat4::from_rotation_y(frame as f32 * 0.01)];
        let mut before = || {
            let start = std::time::Instant::now();
            // The exact pre-change producer loop, retained only for this explicit timing probe.
            crate::geometry::transform(
                &source,
                glam::Mat4::IDENTITY,
                Some(&palette),
                &mut old_scratch,
            );
            let mut bounds = [
                glam30::Vec3::splat(f32::INFINITY),
                glam30::Vec3::splat(f32::NEG_INFINITY),
            ];
            for (vertex, posed) in old.vertices.iter_mut().zip(&old_scratch) {
                vertex.position = posed.position;
                vertex.normal = posed.normal;
                vertex.tangent = posed.tangent;
                let p = glam30::Vec3::from_array(posed.position);
                bounds[0] = bounds[0].min(p);
                bounds[1] = bounds[1].max(p);
            }
            old.bounds = bounds;
            old.revision = old.revision.wrapping_add(1);
            std::hint::black_box(&old);
            start.elapsed().as_secs_f64() * 1000.
        };
        let mut after = || {
            let start = std::time::Instant::now();
            std::hint::black_box(update_skin(&mut new, &source, &palette, &mut new_scratch));
            start.elapsed().as_secs_f64() * 1000.
        };
        let (a, b) = if frame % 2 == 0 {
            (before(), after())
        } else {
            let b = after();
            (before(), b)
        };
        if frame >= 2 {
            old_times.push(a);
            new_times.push(b);
        }
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&old.vertices),
            bytemuck::cast_slice::<_, u8>(&new.vertices)
        );
        assert_eq!(old.bounds, new.bounds);
    }
    old_times.sort_by(f64::total_cmp);
    new_times.sort_by(f64::total_cmp);
    println!(
        "A3 skin 4096 vertices/300 frames: before p50={:.6} p99={:.6} max={:.6} ms; after p50={:.6} p99={:.6} max={:.6} ms; byte-identical all frames",
        old_times[150],
        old_times[296],
        old_times[299],
        new_times[150],
        new_times[296],
        new_times[299]
    );
}
