use crate::{JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn cook(&mut self, shape: Shape<'_>) -> Result<ShapeHandle> {
        let mut d = sys::AraShapeDesc::default();
        let (mut vertices, mut indices, mut heights, mut parts) =
            (&[][..], &[][..], &[][..], &[][..]);
        let (geometry, key) = match shape {
            Shape::Box { half_extents } => {
                d.kind = 0;
                d.size = v(half_extents);
                (
                    DebugGeometry::Box(half_extents),
                    vec![
                        0,
                        half_extents[0].to_bits(),
                        half_extents[1].to_bits(),
                        half_extents[2].to_bits(),
                    ],
                )
            }
            Shape::Sphere { radius } => {
                d.kind = 1;
                d.radius = radius;
                (DebugGeometry::Sphere(radius), vec![1, radius.to_bits()])
            }
            Shape::Capsule {
                radius,
                half_height,
            } => {
                d.kind = 2;
                d.radius = radius;
                d.half_height = half_height;
                (
                    DebugGeometry::Capsule {
                        radius,
                        half_height,
                    },
                    vec![2, radius.to_bits(), half_height.to_bits()],
                )
            }
            Shape::Cylinder {
                radius,
                half_height,
            } => {
                d.kind = 3;
                d.radius = radius;
                d.half_height = half_height;
                (
                    DebugGeometry::Cylinder {
                        radius,
                        half_height,
                    },
                    vec![3, radius.to_bits(), half_height.to_bits()],
                )
            }
            Shape::ConvexHull { vertices: input } => {
                d.kind = 4;
                vertices = input;
                (DebugGeometry::ConvexHull(input.to_vec()), Vec::new())
            }
            Shape::Mesh {
                vertices: input,
                indices: input_indices,
            } => {
                d.kind = 5;
                vertices = input;
                indices = input_indices;
                (
                    DebugGeometry::Mesh {
                        vertices: input.to_vec(),
                        indices: input_indices.to_vec(),
                    },
                    Vec::new(),
                )
            }
            Shape::HeightField {
                side,
                heights: input,
                scale,
                offset,
            } => {
                if side < 4
                    || side.checked_mul(side).map(|n| n as usize) != Some(input.len())
                    || input.len() > 16_777_216
                {
                    return Err(PhysicsError(
                        "heightfield must have side*side samples, side>=4, at most 16M samples"
                            .into(),
                    ));
                }
                d.kind = 6;
                d.sample_count = side;
                d.size = v(scale);
                d.offset = v(offset);
                heights = input;
                let mesh_vertices = (0..input.len())
                    .map(|i| {
                        [
                            offset[0] + scale[0] * (i % side as usize) as f32,
                            offset[1] + scale[1] * input[i],
                            offset[2] + scale[2] * (i / side as usize) as f32,
                        ]
                    })
                    .collect();
                let mut mesh_indices = Vec::new();
                for z in 0..side - 1 {
                    for x in 0..side - 1 {
                        let a = z * side + x;
                        mesh_indices.extend_from_slice(&[
                            a,
                            a + side,
                            a + 1,
                            a + 1,
                            a + side,
                            a + side + 1,
                        ]);
                    }
                }
                (
                    DebugGeometry::Mesh {
                        vertices: mesh_vertices,
                        indices: mesh_indices,
                    },
                    Vec::new(),
                )
            }
            Shape::Compound { parts: input } => {
                d.kind = 7;
                parts = input;
                for p in input {
                    self.require_shape(p.shape)?;
                }
                (DebugGeometry::Compound(input.to_vec()), Vec::new())
            }
        };
        if let Some(handle) = self.primitive_cache.get(&key).filter(|_| !key.is_empty()) {
            return Ok(*handle);
        }
        let raw_vertices: Vec<_> = vertices.iter().copied().map(v).collect();
        let raw_parts: Vec<_> = parts
            .iter()
            .map(|p| sys::AraShapePart {
                shape: p.shape.raw(),
                position: v(p.pose.position),
                rotation: q(p.pose.rotation),
            })
            .collect();
        let (vertex_count, index_count, sample_len, part_count) = (
            Self::count(vertices.len())?,
            Self::count(indices.len())?,
            Self::count(heights.len())?,
            Self::count(parts.len())?,
        );
        // SAFETY: every pointer/count pair is borrowed for this synchronous call.
        // The shim checks indices, shape references and shape-specific dimensions.
        let id = unsafe {
            sys::ara_shape_create(
                self.ptr(),
                &d,
                raw_vertices.as_ptr(),
                vertex_count,
                indices.as_ptr(),
                index_count,
                heights.as_ptr(),
                sample_len,
                raw_parts.as_ptr(),
                part_count,
            )
        };
        if id == sys::INVALID {
            return Err(Self::error());
        }
        let handle = ShapeHandle::from_raw(id);
        self.changed(10 + u64::from(d.kind));
        self.hash_words([u64::from(id), u64::from(d.sample_count)]);
        self.hash_floats(
            a(d.size)
                .into_iter()
                .chain([d.radius, d.half_height])
                .chain(a(d.offset)),
        );
        for p in vertices {
            self.hash_floats(*p);
        }
        self.hash_words(indices.iter().copied().map(u64::from));
        self.hash_floats(heights.iter().copied());
        for p in parts {
            self.hash_words([u64::from(p.shape.raw())]);
            self.hash_floats(p.pose.position.into_iter().chain(p.pose.rotation.0));
        }
        self.shapes.push(geometry);
        if !key.is_empty() {
            self.primitive_cache.insert(key, handle);
        }
        Ok(handle)
    }
}
