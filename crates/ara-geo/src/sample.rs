//! Original procedural benchmark media, dedicated to the public domain (CC0).
use crate::{SourceMesh, Vertex};

/// Generates a closed, indexed faceted rock without duplicated polar vertices.
/// `rings * segments * 2` is approximately the triangle count. No RNG or clock.
pub fn rock(rings: u32, segments: u32) -> SourceMesh {
    let rings = rings.clamp(3, 4096);
    let segments = segments.clamp(3, 4096);
    let mut vertices = vec![Vertex {
        position: [0.0, 1.2, 0.0],
        normal: [0.0, 1.0, 0.0],
        uv: [0.5, 0.0],
    }];
    for r in 1..rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for s in 0..segments {
            let u = s as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let p = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            let radius = 1.0
                + 0.10 * (p[0] * 9.0 + p[1] * 6.0).sin() * (p[2] * 8.0).cos()
                + 0.04 * (p[0] * 31.0 + p[2] * 27.0).cos() * theta.sin();
            vertices.push(Vertex {
                position: [p[0] * radius, p[1] * radius * 1.2, p[2] * radius],
                normal: p,
                uv: [u, v],
            });
        }
    }
    let bottom = vertices.len() as u32;
    vertices.push(Vertex {
        position: [0.0, -1.2, 0.0],
        normal: [0.0, -1.0, 0.0],
        uv: [0.5, 1.0],
    });
    let mut indices = Vec::new();
    for s in 0..segments {
        let next = (s + 1) % segments;
        indices.extend([0, 1 + next, 1 + s]);
        for r in 0..rings - 2 {
            let a = 1 + r * segments + s;
            let b = 1 + r * segments + next;
            indices.extend([a, b, a + segments, b, b + segments, a + segments]);
        }
        let a = 1 + (rings - 2) * segments;
        indices.extend([a + s, a + next, bottom]);
    }
    SourceMesh { vertices, indices }
}
