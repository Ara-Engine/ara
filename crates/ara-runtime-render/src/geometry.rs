//! Model-space geometry (procedural shapes, decoded primitives) and the per-frame
//! transform to world space, with CPU linear-blend skinning for skinned primitives.
//! GPU skinning needs a skinned vertex ABI in the cooked shaders (M3).
use ara_render::scene::Vertex;
use ara_runtime::{components::MeshShape, model::ModelVertex};
use glam::{Mat3, Mat4, Vec3};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Model-space triangles shared by every draw of the same geometry.
pub struct Source {
    /// Vertices with optional skin influences.
    pub vertices: Vec<ModelVertex>,
    /// Triangle list indices.
    pub indices: Vec<u32>,
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2], tangent: Vec3) -> ModelVertex {
    ModelVertex {
        position: position.to_array(),
        normal: normal.to_array(),
        uv,
        tangent: [tangent.x, tangent.y, tangent.z, 1.0],
        ..Default::default()
    }
}

/// A procedural shape of `size` metres, centred on the origin (planes at y = 0).
pub fn shape(shape: MeshShape, size: [f32; 3]) -> Source {
    let half = Vec3::from_array(size) * 0.5;
    match shape {
        MeshShape::Box => cuboid(half),
        MeshShape::Plane => {
            let mut s = Source {
                vertices: Vec::new(),
                indices: vec![0, 2, 1, 0, 3, 2],
            };
            for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                s.vertices.push(vertex(
                    Vec3::new(x * half.x, 0.0, z * half.z),
                    Vec3::Y,
                    [(x + 1.0) * half.x, (z + 1.0) * half.z],
                    Vec3::X,
                ));
            }
            s
        }
        MeshShape::Sphere => capsule(half.x, 0.0),
        MeshShape::Capsule => capsule(half.x, (half.y - half.x).max(0.0)),
    }
}

fn cuboid(half: Vec3) -> Source {
    let mut s = Source {
        vertices: Vec::with_capacity(24),
        indices: Vec::with_capacity(36),
    };
    // (normal, tangent); bitangent = normal × tangent completes a right-handed face.
    let faces = [
        (Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_X, Vec3::Z),
        (Vec3::Y, Vec3::X),
        (Vec3::NEG_Y, Vec3::X),
        (Vec3::Z, Vec3::X),
        (Vec3::NEG_Z, Vec3::NEG_X),
    ];
    for (n, t) in faces {
        let b = n.cross(t);
        let base = s.vertices.len() as u32;
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = (n + t * u + b * v) * half;
            s.vertices
                .push(vertex(p, n, [(u + 1.0) * 0.5, (v + 1.0) * 0.5], t));
        }
        s.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    s
}

/// A capsule (a sphere when `cylinder` is zero): hemispheres of `radius` separated
/// by a cylinder of half height `cylinder`, from rings of a UV sphere.
fn capsule(radius: f32, cylinder: f32) -> Source {
    const SEGMENTS: u32 = 24;
    const RINGS: u32 = 8; // per hemisphere
    let mut s = Source {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let rows = 2 * RINGS + 2;
    for row in 0..rows {
        let (latitude, offset) = if row <= RINGS {
            (FRAC_PI_2 - row as f32 * FRAC_PI_2 / RINGS as f32, cylinder)
        } else {
            (
                -((row - RINGS - 1) as f32) * FRAC_PI_2 / RINGS as f32,
                -cylinder,
            )
        };
        for column in 0..=SEGMENTS {
            let longitude = column as f32 / SEGMENTS as f32 * TAU;
            let n = Vec3::new(
                latitude.cos() * longitude.sin(),
                latitude.sin(),
                latitude.cos() * longitude.cos(),
            );
            let p = n * radius + Vec3::Y * offset;
            let t = Vec3::new(longitude.cos(), 0.0, -longitude.sin());
            let v = 0.5 - p.y / (2.0 * (radius + cylinder)).max(1e-6);
            s.vertices
                .push(vertex(p, n, [column as f32 / SEGMENTS as f32, v], t));
        }
    }
    for row in 0..rows - 1 {
        for column in 0..SEGMENTS {
            let a = row * (SEGMENTS + 1) + column;
            let b = a + SEGMENTS + 1;
            s.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    s
}

/// Writes world-space vertices of `source` into `out`: `world × skin` per vertex,
/// where `skin` blends up to eight palette matrices (skin joint order).
pub fn transform(
    source: &[ModelVertex],
    world: Mat4,
    palette: Option<&[Mat4]>,
    out: &mut Vec<Vertex>,
) {
    out.clear();
    out.reserve(source.len());
    let rigid = Mat3::from_mat4(world);
    for v in source {
        let (m, m3) = match palette {
            Some(palette) if v.weights.iter().any(|w| *w > 0.0) => {
                let mut skin = Mat4::ZERO;
                for (&joint, &weight) in v.joints.iter().zip(&v.weights) {
                    if weight > 0.0
                        && let Some(matrix) = palette.get(joint as usize)
                    {
                        skin += *matrix * weight;
                    }
                }
                let m = world * skin;
                (m, Mat3::from_mat4(m))
            }
            _ => (world, rigid),
        };
        let normal = (m3 * Vec3::from_array(v.normal)).normalize_or(Vec3::Y);
        let tangent = (m3 * Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]))
            .normalize_or(normal.any_orthonormal_vector());
        out.push(Vertex {
            position: m.transform_point3(Vec3::from_array(v.position)).to_array(),
            normal: normal.to_array(),
            uv: v.uv,
            tangent: [tangent.x, tangent.y, tangent.z, v.tangent[3]],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_are_closed_and_sized() {
        for kind in [
            MeshShape::Box,
            MeshShape::Sphere,
            MeshShape::Capsule,
            MeshShape::Plane,
        ] {
            let s = shape(kind, [1.0, 2.0, 1.0]);
            assert!(s.indices.len().is_multiple_of(3) && !s.vertices.is_empty());
            assert!(s.indices.iter().all(|&i| (i as usize) < s.vertices.len()));
        }
        let capsule = shape(MeshShape::Capsule, [1.0, 2.0, 1.0]);
        let top = capsule
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!((top - 1.0).abs() < 1e-5, "capsule spans its height");
        let mut out = Vec::new();
        transform(
            &capsule.vertices,
            Mat4::from_translation(Vec3::X),
            None,
            &mut out,
        );
        assert!((out[0].position[0] - 1.0).abs() < 1e-5);
    }
}
