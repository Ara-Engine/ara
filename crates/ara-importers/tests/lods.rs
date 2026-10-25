//! Classic LOD depth, seam isolation, source identity and model-space error scale.
#![allow(clippy::unwrap_used)]
use ara_asset_format::Mesh;
use ara_importers::{MeshSettings, Vertex, cook_mesh};

fn rows(bytes: &[u8]) -> Vec<(usize, usize, f32)> {
    Mesh::read(bytes)
        .unwrap()
        .lods
        .as_chunks::<16>()
        .0
        .iter()
        .map(|row| {
            (
                u32::from_le_bytes(row[..4].try_into().unwrap()) as usize,
                u32::from_le_bytes(row[4..8].try_into().unwrap()) as usize,
                f32::from_le_bytes(row[8..12].try_into().unwrap()),
            )
        })
        .collect()
}

fn sphere() -> (Vec<Vertex>, Vec<u32>) {
    let (columns, rings) = (64u32, 32u32);
    let mut vertices = Vec::new();
    for y in 0..=rings {
        for x in 0..=columns {
            let latitude = y as f32 / rings as f32 * std::f32::consts::PI;
            let longitude = (x % columns) as f32 / columns as f32 * std::f32::consts::TAU;
            let position = if y == 0 {
                [0.0, 1.0, 0.0]
            } else if y == rings {
                [0.0, -1.0, 0.0]
            } else {
                [
                    latitude.sin() * longitude.cos(),
                    latitude.cos(),
                    latitude.sin() * longitude.sin(),
                ]
            };
            vertices.push(Vertex {
                position,
                normal: position,
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv0: [x as f32 / columns as f32, y as f32 / rings as f32],
                color: [1.0; 4],
                source_index: vertices.len() as u32,
                ..Vertex::default()
            });
        }
    }
    let mut indices = Vec::new();
    for y in 0..rings {
        for x in 0..columns {
            let a = y * (columns + 1) + x;
            let b = a + columns + 1;
            if y > 0 {
                indices.extend([a, a + 1, b]);
            }
            if y + 1 < rings {
                indices.extend([a + 1, b + 1, b]);
            }
        }
    }
    (vertices, indices)
}

#[test]
fn static_chains_reach_the_coarse_goal_with_useful_monotone_levels() {
    let (vertices, indices) = sphere();
    let bytes = cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap();
    let levels = rows(&bytes);
    assert!(levels.len() > 4, "{levels:?}");
    assert!(
        levels.iter().take(4).all(|l| l.2 <= 0.01),
        "retain the usable geometric-error prefix: {levels:?}"
    );
    assert!(
        levels.last().unwrap().1 / 3 <= (indices.len() / 3).div_ceil(50).max(200),
        "{levels:?}"
    );
    assert_eq!(levels[0], (0, indices.len(), 0.0));
    assert!(
        levels
            .windows(2)
            .all(|pair| pair[1].1 * 4 <= pair[0].1 * 3 && pair[1].2 >= pair[0].2)
    );
    assert!(levels.iter().all(|l| l.1 > 0 && l.2.is_finite()));
    assert_eq!(
        bytes,
        cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap()
    );
    let one = cook_mesh(
        &vertices,
        &indices,
        MeshSettings {
            lod_count: 1,
            ..MeshSettings::default()
        },
    )
    .unwrap();
    assert_eq!(rows(&one).len(), 1, "honour an explicitly disabled chain");
}

#[test]
fn unindexed_static_geometry_gets_a_chain_instead_of_a_single_locked_lod() {
    let n = 17u32;
    let mut vertices = Vec::new();
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            for (x, y) in [
                (x, y),
                (x + 1, y),
                (x, y + 1),
                (x + 1, y),
                (x + 1, y + 1),
                (x, y + 1),
            ] {
                vertices.push(Vertex {
                    position: [x as f32, y as f32, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv0: [x as f32 / 16.0, y as f32 / 16.0],
                    color: [1.0; 4],
                    source_index: vertices.len() as u32,
                    ..Vertex::default()
                });
            }
        }
    }
    let indices: Vec<_> = (0..vertices.len() as u32).collect();
    let bytes = cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap();
    let levels = rows(&bytes);
    assert!(levels.len() > 1, "{levels:?}");
    assert!(levels.last().unwrap().1 / 3 <= 200, "{levels:?}");
}

#[test]
fn cooked_relative_error_converts_to_model_metres_under_uniform_scale() {
    let (vertices, indices) = sphere();
    let a = rows(&cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap());
    let scaled: Vec<_> = vertices
        .iter()
        .map(|v| Vertex {
            position: v.position.map(|p| p * 8.0),
            ..*v
        })
        .collect();
    let b = rows(&cook_mesh(&scaled, &indices, MeshSettings::default()).unwrap());
    assert_eq!(a.len(), b.len());
    assert!(a.iter().skip(1).any(|row| row.2 > 0.0));
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a.1, b.1);
        assert!((a.2 - b.2).abs() < 1e-6);
        // ARAF v1 stores relative error; native presentation multiplies by the
        // model extent, then instance scale and projected pixels per metre.
        let (metres_a, metres_b) = (a.2 * 2.0, b.2 * 16.0);
        assert!((metres_b - metres_a * 8.0).abs() < 1e-5);
    }
}

#[test]
fn skinned_geometry_keeps_the_conservative_deformation_policy() {
    let (mut vertices, indices) = sphere();
    for vertex in &mut vertices {
        vertex.weights[0] = 1.0;
    }
    let levels = rows(&cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap());
    assert!(levels.len() > 1);
    assert!(levels.len() <= 4, "{levels:?}");
    assert!(levels.iter().all(|row| row.2 <= 0.01), "{levels:?}");
}

#[test]
fn uv_islands_do_not_mix_even_with_permissive_simplification() {
    let n = 17u32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for island in 0..2u32 {
        let base = vertices.len() as u32;
        for y in 0..n {
            for x in 0..n {
                vertices.push(Vertex {
                    position: [(x + island * (n - 1)) as f32, y as f32, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv0: [
                        x as f32 / (n - 1) as f32 + island as f32 * 2.0,
                        y as f32 / (n - 1) as f32,
                    ],
                    color: [1.0; 4],
                    source_index: vertices.len() as u32,
                    ..Vertex::default()
                });
            }
        }
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = base + y * n + x;
                indices.extend([a, a + 1, a + n, a + 1, a + n + 1, a + n]);
            }
        }
    }
    let bytes = cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap();
    let mesh = Mesh::read(&bytes).unwrap();
    let levels = rows(&bytes);
    assert!(levels.len() > 1);
    let sources: Vec<_> = mesh
        .source_vertices
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    let all: Vec<_> = mesh
        .indices
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    for &(first, count, _) in &levels {
        for tri in all[first..first + count].as_chunks::<3>().0 {
            let islands: Vec<_> = tri.iter().map(|&i| sources[i as usize] / (n * n)).collect();
            assert!(
                islands.iter().all(|i| *i == islands[0]),
                "UV seam crossed: {islands:?}"
            );
        }
    }
}
