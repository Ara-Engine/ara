use crate::{Mesh, Settings, Triangle, build, cache};

fn square() -> (Vec<[f32; 3]>, Vec<Triangle>) {
    (
        vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
        vec![
            Triangle {
                vertices: [0, 1, 2],
                material: 3,
                albedo: [0.4; 3],
            },
            Triangle {
                vertices: [0, 2, 3],
                material: 3,
                albedo: [0.4; 3],
            },
        ],
    )
}

#[test]
fn charts_are_bit_identical_across_runs_and_thread_counts() {
    let (positions, triangles) = square();
    let mesh = Mesh {
        positions: &positions,
        triangles: &triangles,
    };
    let settings = Settings {
        scale: [1.33, -0.91, 2.7],
        ..Settings::default()
    };
    let key = cache::Key::new(mesh, settings);
    let reference = cache::encode(&build(mesh, settings).unwrap(), key);
    for threads in [1, 2, 4, 8] {
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..3 {
                            assert_eq!(
                                cache::encode(&build(mesh, settings).unwrap(), key),
                                reference
                            );
                        }
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }
}

#[test]
fn square_distribution_coverage_and_nonuniform_scale() {
    let (positions, triangles) = square();
    for (scale, expected) in [([1.; 3], 16), ([2., 0.5, 1.], 16), ([-2., 0.5, 1.], 16)] {
        let charts = build(
            Mesh {
                positions: &positions,
                triangles: &triangles,
            },
            Settings {
                scale,
                ..Settings::default()
            },
        )
        .unwrap();
        assert_eq!(charts.patches.len(), expected);
        let mut areas = Vec::new();
        let mut edges = Vec::new();
        for patch in &charts.patches {
            areas.push(patch.area);
            edges.push(
                (0..3)
                    .map(|i| patch.max[i] - patch.min[i])
                    .fold(0.0f32, f32::max),
            );
            assert!((patch.area - 0.0625).abs() < 1e-6);
            assert_eq!(patch.normal[2].signum(), scale[0].signum());
            let mut fragment_area = 0.0;
            for fragment in charts.coverage(patch) {
                let source = triangles[fragment.triangle as usize]
                    .vertices
                    .map(|i| positions[i as usize]);
                let p = fragment.barycentric.map(|[u, v]| {
                    std::array::from_fn::<_, 3, _>(|i| {
                        f64::from(
                            (source[0][i] * (1. - u - v) + source[1][i] * u + source[2][i] * v)
                                * scale[i],
                        )
                    })
                });
                let n =
                    crate::clip::cross(crate::clip::sub(p[1], p[0]), crate::clip::sub(p[2], p[0]));
                fragment_area += crate::clip::dot(n, n).sqrt() * 0.5;
            }
            assert!((fragment_area - f64::from(patch.area)).abs() < 1e-6);
        }
        areas.sort_by(f32::total_cmp);
        edges.sort_by(f32::total_cmp);
        println!(
            "square scale={scale:?}: patches={expected}, edge min/median/max={}/{}/{}, area min/median/max={}/{}/{}",
            edges[0],
            edges[edges.len() / 2],
            edges[edges.len() - 1],
            areas[0],
            areas[areas.len() / 2],
            areas[areas.len() - 1]
        );
        assert_eq!(
            charts.resident_bytes(),
            expected * size_of::<crate::Patch>()
                + charts.fragments.len() * size_of::<crate::Fragment>()
        );
    }
}

#[test]
fn material_seams_creases_and_disconnected_surfaces_never_merge() {
    let positions = [
        [0., 0., 0.],
        [0.2, 0., 0.],
        [0.2, 0.2, 0.],
        [0., 0.2, 0.],
        [0.2, 0.2, 0.2],
    ];
    let triangles = [
        Triangle {
            vertices: [0, 1, 2],
            material: 0,
            albedo: [0.5; 3],
        },
        Triangle {
            vertices: [0, 2, 3],
            material: 1,
            albedo: [0.7; 3],
        },
        Triangle {
            vertices: [1, 4, 2],
            material: 0,
            albedo: [0.5; 3],
        },
    ];
    let mesh = |triangles| Mesh {
        positions: &positions,
        triangles,
    };
    let charts = build(mesh(&triangles), Settings::default()).unwrap();
    assert_eq!(charts.patches.len(), 3);
    for patch in &charts.patches {
        assert!(
            charts
                .coverage(patch)
                .iter()
                .all(|f| triangles[f.triangle as usize].material == patch.material)
        );
    }
    let mut merged = triangles;
    merged[1].material = 0;
    assert_eq!(
        build(mesh(&merged), Settings::default())
            .unwrap()
            .patches
            .len(),
        2
    );
    let positions = [
        [0., 0., 0.],
        [0.05, 0., 0.],
        [0., 0.05, 0.],
        [0.1, 0.1, 0.],
        [0.15, 0.1, 0.],
        [0.1, 0.15, 0.],
    ];
    let triangles = [
        Triangle {
            vertices: [0, 1, 2],
            material: 0,
            albedo: [0.5; 3],
        },
        Triangle {
            vertices: [3, 4, 5],
            material: 0,
            albedo: [0.5; 3],
        },
    ];
    assert_eq!(
        build(
            Mesh {
                positions: &positions,
                triangles: &triangles
            },
            Settings::default()
        )
        .unwrap()
        .patches
        .len(),
        2
    );
}

#[test]
fn cache_roundtrip_misses_and_corruption_fall_back() {
    let (positions, triangles) = square();
    let mesh = Mesh {
        positions: &positions,
        triangles: &triangles,
    };
    let settings = Settings::default();
    let cold = cache::prepare(mesh, settings, None).unwrap();
    let bytes = cold.write_back.unwrap();
    let warm = cache::prepare(mesh, settings, Some(&bytes)).unwrap();
    assert_eq!(cold.charts, warm.charts);
    assert!(warm.write_back.is_none());
    let mut broken = bytes.clone();
    broken[130] ^= 1;
    assert_eq!(
        cache::prepare(mesh, settings, Some(&broken))
            .unwrap()
            .write_back
            .unwrap(),
        bytes
    );
    for length in [0, 7, 103, bytes.len() - 1] {
        assert!(cache::decode(&bytes[..length], cold.key, settings).is_err());
    }
    let rescaled = cache::prepare(
        mesh,
        Settings {
            scale: [2.; 3],
            ..settings
        },
        Some(&bytes),
    )
    .unwrap();
    assert!(rescaled.write_back.is_some());
    assert_ne!(cold.key, rescaled.key);
}

#[test]
fn invalid_inputs_and_limits_are_errors_not_quality_reductions() {
    let (positions, mut triangles) = square();
    assert!(
        build(
            Mesh {
                positions: &positions,
                triangles: &triangles
            },
            Settings {
                max_fragments: 1,
                ..Settings::default()
            }
        )
        .is_err()
    );
    for scale in [[0.; 3], [f32::INFINITY; 3]] {
        assert!(
            build(
                Mesh {
                    positions: &positions,
                    triangles: &triangles
                },
                Settings {
                    scale,
                    ..Settings::default()
                }
            )
            .is_err()
        );
    }
    triangles[0].vertices[0] = 99;
    assert!(
        build(
            Mesh {
                positions: &positions,
                triangles: &triangles
            },
            Settings::default()
        )
        .is_err()
    );
}

#[test]
fn clipped_fp32_area_underflow_does_not_reject_the_representable_surface() {
    // A metre-scale triangle crossing z=0 has a representable main area,
    // but its positive-z tip clips to an area below FP32's smallest subnormal.
    let positions = [[0., 0., 1e-23], [1., 0., -1.], [0., 1., -1.]];
    let triangles = [Triangle {
        vertices: [0, 1, 2],
        material: 0,
        albedo: [0.5; 3],
    }];
    let charts = build(
        Mesh {
            positions: &positions,
            triangles: &triangles,
        },
        Settings::default(),
    )
    .unwrap();
    let p = positions.map(|p| p.map(f64::from));
    let n = crate::clip::cross(crate::clip::sub(p[1], p[0]), crate::clip::sub(p[2], p[0]));
    let expected = crate::clip::dot(n, n).sqrt() * 0.5;
    let actual: f64 = charts.patches.iter().map(|p| f64::from(p.area)).sum();
    assert!(
        (actual / expected - 1.).abs() < 1e-5,
        "{actual:e} vs {expected:e}"
    );
    assert!(charts.patches.iter().all(|p| p.area > 0.));
    assert_eq!(
        charts
            .patches
            .iter()
            .map(|p| p.fragment_count as usize)
            .sum::<usize>(),
        charts.fragments.len()
    );
}

#[test]
fn oblique_triangles_conserve_area_across_grid_planes_and_mirrored_scale() {
    let mut seed = 17u64;
    let mut coordinate = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 40) as i32 - (1 << 23)) as f32 / (1 << 22) as f32
    };
    for _ in 0..96 {
        let positions: [[f32; 3]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| coordinate()));
        let triangles = [Triangle {
            vertices: [0, 1, 2],
            material: 9,
            albedo: [0.3, 0.7, 0.1],
        }];
        let settings = Settings {
            scale: [-0.7, 1.3, 1.1],
            ..Settings::default()
        };
        let charts = build(
            Mesh {
                positions: &positions,
                triangles: &triangles,
            },
            settings,
        )
        .unwrap();
        let p = positions
            .map(|p| std::array::from_fn(|i| f64::from(p[i]) * f64::from(settings.scale[i])));
        let n = crate::clip::cross(crate::clip::sub(p[1], p[0]), crate::clip::sub(p[2], p[0]));
        let area = crate::clip::dot(n, n).sqrt() * 0.5;
        let actual: f64 = charts.patches.iter().map(|p| f64::from(p.area)).sum();
        assert!((actual / area - 1.).abs() < 2e-6, "{actual} vs {area}");
        for patch in charts.patches.iter() {
            assert!(
                patch
                    .normal
                    .iter()
                    .zip(n)
                    .map(|(&v, n)| f64::from(v) * n / (area * 2.))
                    .sum::<f64>()
                    > 0.99999
            );
            assert!((0..3).all(|i| patch.max[i] - patch.min[i] <= 0.2500003));
        }
    }
}
