//! Recognize thin cards in a component-local frame, independent of rotation.
use super::Vertex;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn classify(
    vertices: &[Vertex],
    triangles: &[[u32; 3]],
    components: &[usize],
    count: usize,
) -> Vec<bool> {
    let mut origins = vec![None; count];
    let mut along = vec![[0.0; 3]; count];
    for (triangle, &c) in triangles.iter().zip(components) {
        for &v in triangle {
            let p = vertices[v as usize].position.map(f64::from);
            let origin = origins[c].get_or_insert(p);
            let delta = std::array::from_fn(|a| p[a] - origin[a]);
            if dot(delta, delta) > dot(along[c], along[c]) {
                along[c] = delta;
            }
        }
    }
    for axis in &mut along {
        let length = dot(*axis, *axis).sqrt();
        if length > 0.0 {
            *axis = axis.map(|v| v / length);
        }
    }
    let mut across = vec![[0.0; 3]; count];
    for (triangle, &c) in triangles.iter().zip(components) {
        let origin = origins[c].expect("each component has a triangle");
        for &v in triangle {
            let p = vertices[v as usize].position.map(f64::from);
            let delta = std::array::from_fn(|a| p[a] - origin[a]);
            let projection = dot(delta, along[c]);
            let radial = std::array::from_fn(|a| delta[a] - along[c][a] * projection);
            if dot(radial, radial) > dot(across[c], across[c]) {
                across[c] = radial;
            }
        }
    }
    let normals: Vec<_> = along
        .iter()
        .zip(&across)
        .map(|(&a, &b)| {
            let normal = cross(a, b);
            let length = dot(normal, normal).sqrt();
            if length > 0.0 {
                normal.map(|v| v / length)
            } else {
                [0.0; 3]
            }
        })
        .collect();
    let mut thickness = vec![0.0f64; count];
    for (triangle, &c) in triangles.iter().zip(components) {
        let origin = origins[c].expect("each component has a triangle");
        for &v in triangle {
            let p = vertices[v as usize].position.map(f64::from);
            let delta = std::array::from_fn(|a| p[a] - origin[a]);
            thickness[c] = thickness[c].max(dot(delta, normals[c]).abs());
        }
    }
    across
        .iter()
        .zip(thickness)
        .map(|(&width, thickness)| {
            let width_squared = dot(width, width);
            width_squared > 0.0 && thickness * thickness <= width_squared * 0.05 * 0.05
        })
        .collect()
}
